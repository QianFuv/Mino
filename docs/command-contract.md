# Mino CLI 与 JSON 契约

本文是当前实现的权威命令清单，面向编写集成、调试状态转换和核对机器输出的读者。单个参数的完整拼写和值域以递归 `mino --help` 为准；本文重点说明跨命令保持稳定的行为约束。

当前 crate 版本为 `0.1.0`。命令是否受支持以本清单、递归 `--help` 和 `mino agent capabilities` 的一致结果为准。

## 调用约定

全局选项可以放在子命令之前或之后：

| 选项 | 行为 |
|---|---|
| `--root <path>` | 从指定文件或目录开始发现项目；默认使用当前目录。 |
| `--format human|json` | 选择单行人工输出或版本化机器输出；默认 `human`。 |
| `--no-input` | 禁止交互输入；所有 `agent` 命令都必须使用。 |

代理集成必须同时提供 `--format json` 和 `--no-input`。每次调用只输出一个 UTF-8 JSON value，并以换行结束。成功的机器输出写 stdout；能够形成 Mino 结果的失败也写 stdout，便于调用方解析。只有无法形成结果 envelope 的参数解析诊断才写 stderr。不要合并两个 stream。

### Revision 与幂等性

- `plan create` 创建新计划，不接受 `--expect-revision`，但要求 UUID `--request-id`。
- `plan fork` 以 `--from-revision` 绑定历史来源，同样要求 request ID，但不接受目标计划 revision。
- 修改现有计划或证据的命令要求当前 `--expect-revision` 和 UUID `--request-id`；authored mutation 还要求 `--actor`。
- 每项不同修改必须使用新的 UUID；只有输入完全相同时才能复用 request ID 进行重试。
- 修改成功后必须丢弃旧 revision，重新读取 context。
- `git bind`、`git branch create` 和自动 `git commit` 不接受调用方自定义的计划 mutation 元数据。`git commit record-manual` 与 `git gate skip` 是显式的 revision/request mutation，并要求 approval reference。

Agent schema 公开稳定的 `executor_identity: "codex"`。Mino 返回的 revisioned mutation `next_actions[].argv` 总是显式包含 `--actor codex`；只读 argv 不包含 actor。人工直接输入 mutation 命令且省略该选项时仍使用 `--actor user` 默认值。调用方必须执行数组 argv，不能删除、替换或从会话身份猜测 actor。

## 命令总览

### 项目发现与协议

| 命令 | 写入范围 | 核心契约 |
|---|---|---|
| `mino project init` | 项目本地文件 | 创建缺失的 `.mino` 状态，在分类前恢复摘要可证明的集成替换，并安装或验证 Skill；新的受管区块修改必须显式选择应用。 |
| `mino project show` | 无 | 返回配置、锁和 doctor findings。 |
| `mino project doctor` | 无 | 诊断协议锁、事务、投影、Skill 和受管区块。 |
| `mino project scan` | 无 | 生成尊重根/嵌套 `.gitignore`、repository/global exclude 和资源预算的工作区与语言证据。 |
| `mino project migrate legacy` | 无 | 分析旧 AGENTS、模板和执行文档，返回映射建议，不改源文件。 |
| `mino project import legacy` | 新 Draft | 读取一个旧计划，把受支持的 authored fields 写入独立 Draft，并报告全部忽略项和警告。 |
| `mino protocol status` | 无 | 校验内嵌资源摘要及项目 protocol lock 兼容性。 |
| `mino protocol migrate` | 存在注册 transform 时修改计划 | 要求目标、revision 和 request ID；当前版本只实现 already-current no-op。 |

`project init --apply-agents-block` 与 `--apply-gitignore-block` 只发起各自 marker-owned 区域的新修改。已存在的 `.mino/integration-transactions/**` 会先按摘要恢复，无需重复 apply flag；无法证明安全的残留会阻止 init 并保留现场。`project doctor` 只报告 pending/corrupt transaction，写入范围仍为“无”。旧计划导入要求项目没有活动计划和同名计划；成功结果仍为 `complete: false`，并明确给出 `draft_review_required: true`、`source_preserved: true`、`historical_execution_trusted: false`。历史 lifecycle、approval、check、commit 和 evidence 结果不会进入新聚合。

`project scan` 的 `files_scanned` 统计进入证据模型的普通非生成文件，`bytes_read` 统计源码与 CI 证据实际读取的内容字节。`truncated` 为 true 时，`truncation_reasons` 是以下稳定代码的排序集合：`depth_limit`、`file_limit`、`per_file_byte_limit`、`total_byte_limit`。截断结果仍保持确定性，但只代表预算内的部分仓库证据。

### Draft 编写、校验与批准

| 命令 | 写入 | 核心契约 |
|---|---:|---|
| `mino plan create` | 是 | 从显式 request 文件、stdin 或有界向导创建 revision 1 Draft，并确定性派生 ASCII Plan ID。 |
| `mino plan metadata set` | 是 | 替换给定的 Draft metadata 字段。 |
| `mino plan summary set` | 是 | 从参数或 stdin 设置 Draft 摘要。 |
| `mino plan context add` | 是 | 追加一组 reference、fact 和 implication。 |
| `mino plan scope set` | 是 | 替换目标、交付物、in-scope 或 out-of-scope。 |
| `mino plan scope add` | 是 | 向一个 scope 列表追加单项。 |
| `mino plan decision add` | 是 | 追加 decision、assumption 或 question。 |
| `mino plan decision update` | 是 | 按 revision 校验后的 1-based 位置完整替换 decision、assumption 或 question。 |
| `mino plan decision remove` | 是 | 按 revision 校验后的 1-based 位置删除 decision、assumption 或 question。 |
| `mino plan edge-case update` | 是 | 按 revision 校验后的 1-based 位置完整替换 edge case。 |
| `mino plan edge-case remove` | 是 | 按 revision 校验后的 1-based 位置删除 edge case。 |
| `mino plan task add` | 是 | 追加下一个确定 ID 的任务及可选 commit gate。 |
| `mino plan task update` | 是 | 按稳定 Task ID 替换给定标题、依赖或 commit gate 字段。 |
| `mino plan task remove` | 是 | 按稳定 Task ID 删除没有被其他任务依赖的任务。 |
| `mino plan task move` | 是 | 在不破坏依赖顺序时，把任务移到给定 1-based 实现位置。 |
| `mino plan task step add` | 是 | 向任务追加有序实现步骤。 |
| `mino plan task step update` | 是 | 按任务内 1-based 位置替换实现步骤。 |
| `mino plan task step remove` | 是 | 按任务内 1-based 位置删除实现步骤。 |
| `mino plan task criterion add` | 是 | 追加下一个确定 ID 的验收条件。 |
| `mino plan task criterion update` | 是 | 按稳定 Criterion ID 替换验收条件描述。 |
| `mino plan task criterion remove` | 是 | 按稳定 Criterion ID 删除验收条件。 |
| `mino plan task verification add` | 是 | 追加任务级计划检查。 |
| `mino plan task verification update` | 是 | 按稳定 Check ID 替换任务级检查定义，不能更改 ID。 |
| `mino plan task verification remove` | 是 | 按稳定 Check ID 删除任务级检查。 |
| `mino plan file add` | 是 | 追加一项由任务负责的 File Map 记录。 |
| `mino plan file update` | 是 | 按任务内 1-based 位置替换 File Map 责任并同步计划总表。 |
| `mino plan file remove` | 是 | 按任务内 1-based 位置删除 File Map 责任并同步计划总表。 |
| `mino plan verification add` | 是 | 追加全局计划检查。 |
| `mino plan verification update` | 是 | 按稳定 Check ID 替换全局检查定义，不能更改 ID。 |
| `mino plan verification remove` | 是 | 按稳定 Check ID 删除全局检查。 |
| `mino plan apply` | 是 | 严格应用一份有界 YAML Draft；拒绝未知字段。 |
| `mino plan next` | 否 | 返回缺失字段和规范修复动作。 |
| `mino plan validate` | 否 | 固定顺序运行 schema、semantic、graph 和 policy 校验。 |
| `mino plan show` | 否 | 返回经过验证的完整规范计划。 |
| `mino plan finalize` | 是 | 校验完整 Draft，并转换为 Ready。 |
| `mino plan review` | 否 | 返回绑定当前 revision 与状态哈希的审批摘要。 |
| `mino plan approve` | 是，审批边界 | 记录显式计划批准，并记录 Approved 或 Disabled Git Flow consent。 |
| `mino plan outcome set` | 是 | 在任务、commit gate 与全局检查完成后写入非空 summary、显式 remaining risk 和可选 follow-up；Review 产生的 follow-up 自动保留来源 `REV-n`。 |
| `mino plan scan accept` | 是，审批边界 | 接受当前精确扫描摘要；仅适用于仍未接受的截断扫描，保存 scan digest、actor、decision reference、reason 与时间。 |

直接 authored 修改只允许发生在 Draft。没有持久化 ID 的列表使用 1-based 位置，并始终与 `--expect-revision` 一起校验；已有 Task、Criterion 或 Check ID 的实体使用稳定 ID。位置过期、目标缺失、依赖顺序被破坏或替换定义不完整时，状态、revision 和投影都保持不变。Ready 或 In Progress 计划必须使用类型化 amendment；任意 JSON path、未知字段和 execution state 字段都会被拒绝。Ready 计划发生 authored 变化后，其旧批准不再有效。

<!-- doc-contract: non-ascii-plan-id -->

`plan create --name` 的显示名称完整保留 UTF-8。名称含 ASCII 字母或数字时继续生成最长 96 字符的现有 slug；纯非 ASCII/标点名称使用原始 UTF-8 字节 SHA-256 的前 8 个小写十六进制字符作为 `plan-<8hex>` slug，完整 ID 为 `YYYY-MM-DD-plan-<8hex>`。相同名称和创建日期得到同一候选 ID，已有同 ID 计划仍按 collision 拒绝，不会覆盖。

批准记录是可审计声明，不是加密签名，也不授权计划之外的文件、网络、部署、消息或 Git 操作。

### 修订、方案比较与归档

| 命令 | 写入 | 核心契约 |
|---|---:|---|
| `mino plan amend propose` | 是 | 保存类型化 patch、基准 revision/hash、计算出的影响和递增 `C<n>`；调用方可提高但不能降低最低分类。 |
| `mino plan amend approve` | 是，审批边界 | 用 `--change C<n> --approval-ref <ref>` 批准当前待处理的 Material 修订。 |
| `mino plan amend reject` | 是，审批边界 | 用 decision reference 和 reason 拒绝尚未批准的 Material 修订，不应用其操作。 |
| `mino plan amend withdraw` | 是 | 原提案人用 reason 撤回尚未批准的 Minor 或 Material 修订，不应用其操作。 |
| `mino plan amend cancel` | 是，审批边界 | 原批准人用 decision reference 和 reason 取消已批准但尚未应用的 Material 修订。 |
| `mino plan amend apply` | 是 | 原子应用符合条件的提案，并按 Minor 或 Material 规则使旧状态失效。 |
| `mino plan fork` | 新 Draft | 审计指定历史 revision，复制 authored values，记录 lineage，并清除执行与信任状态。 |
| `mino plan diff` | 否 | 比较两个当前或历史版本，输出 `mino.plan-diff/v1` 的 Added、Removed、Changed、Moved 路径。 |
| `mino plan alternatives` | 否 | 返回 project selection revision、当前 selected plan 和稳定排序的 live alternatives；旧项目没有 selection 文件时以 revision 0 呈现。 |
| `mino plan select` | selection，审批边界 | 以 `--expect-selection-revision` 和 request ID 选择一个 live alternative，并保存 actor、approval reference、reason 与时间；精确重试不重复递增 revision。 |
| `mino plan archive` | 是，审批边界 | 保存 reason 和 approval reference，以 overlay 停用计划，不删除历史或改变 lifecycle status。 |

<!-- doc-contract: material-amendment-operations -->

`Minor` 只覆盖不会改变用户可见行为的任务局部支持文件、fixture、snapshot、barrel export、检查命令修正和实现说明。`add-task-file` 若引入当前 Standards 未覆盖的 Rust、Python 或 TypeScript/JavaScript 文件，会自动提升最低分类为 `Material`。公开 API、schema、依赖、兼容性、范围、安全约束和核心任务顺序属于 `Material`。

Material patch 还支持 `add-task`、`update-task-definition`、`remove-task`、`replace-task-dependencies`，验收条件的 add/update/remove，任务及全局 verification 的 add/update/remove，以及 `replace-commit-gate`/`remove-commit-gate`。所有 ID、task order、dependency graph、File Map、check uniqueness 和 commit scope 在完整 candidate 上原子校验；删除被引用节点、重复 ID、依赖环或空掉必需执行图会使 apply 无 revision/state 改动地失败。

Material apply 会清除计划批准与 Git consent，重置任务、检查和 commit gate，移除 execution-only checkpoints，把相关证据标为 stale，并要求重新校验和批准。

fork 只读取经过审计的不可变 source snapshot。新计划保留原始需求、范围、决策、标准、任务、检查和提交意图，但清除 lifecycle、审批、amendment、review、evidence、result、execution extension、Git readiness、final outcome 与 archive state。`plan diff` 只比较 authored values；Mino 不提供 plan merge。fork 后原 selected plan 保持不变，新 Draft 进入 alternatives；选中另一个方案前不能归档当前 selected plan。

普通 `plan create` 和 `project import legacy` 在 Git 与非 Git 项目中都拒绝已有 live candidate。显式 `plan fork` 可以为比较创建并存候选；`agent context`/`agent next` 返回 `plan_selection`、候选操作和审批边界，不再因多个方案失败。旧项目没有 selection 文件且只有一个 live plan 时会虚拟选择它；有多个 live plan 时保持 revision 0 且要求显式 `plan select`。Git binding 只描述 worktree identity，不参与项目方案选择。

### 标准检测、目录与冲突

| 命令 | 写入范围 | 核心契约 |
|---|---|---|
| `mino standards detect` | 无 | 从 scanner evidence 返回受支持语言。 |
| `mino standards recommend` | 无 | 推荐 Common 和适用语言包，可按 File Map 缩小范围。 |
| `mino standards apply` | 可选：计划 | 始终要求 `--recommended --seed-verification`。没有 `--plan` 时只读解析；带 `--plan --expect-revision --request-id` 时从完整 File Map 重扫，原子写回内嵌 package、catalog-owned check、扫描摘要和 conflict snapshot，并保留自定义 check。 |
| `mino standards sync` | cache 与 lock | 显式获取并激活摘要校验的目录；当前要求 `--all`。 |
| `mino standards catalog init` | source tree | 在 DNS-like namespace 与 HTTPS base URL 下原子创建惰性示例；不覆盖现有路径。 |
| `mino standards catalog validate` | 无 | 校验路径、SemVer、namespace、TOML、大小和规范身份。 |
| `mino standards catalog build` | static output | 生成并验证 `catalog.toml`、package 文档和 `catalog-manifest.json` 后原子发布。 |
| `mino standards conflict list` | 无 | 列出冲突候选、来源等级、优先级、摘要和决策状态。 |
| `mino standards conflict refresh` | 计划 | 把当前候选集合的指纹写入计划，不选择值。 |
| `mino standards conflict resolve` | 计划，审批边界 | 选择一个当前候选，并记录理由与可审计决策引用。 |

detect、recommend 和 apply 只使用内嵌或已缓存数据；只有 sync 使用配置的网络目录。plan-scoped apply 会用嵌入目录识别所有 catalog-owned check：定义不变时保留现有状态与证据，定义变化或新加入时以无证据 Pending check 替换；不属于目录的自定义 check 不受影响。冲突优先级依次为当前用户要求、仓库规则或本地声明、项目配置、语言包、Common。最高优先级默认值只用于展示，不会被静默应用。所有当前冲突都必须有与来源指纹绑定的显式选择，计划才能通过校验。远程 Team Catalog package 当前只能被 `sync` 验证并缓存，不能被 recommend 或 plan-scoped apply 选择。

<!-- doc-contract: standards-reconciliation-action -->

Validation/Agent 对阻塞 finding 使用精确修复映射：所有非 conflict 的 `POLICY-STANDARD-*` 以及 `POLICY-TOOL-UNAVAILABLE` 返回带 `--plan --expect-revision --request-id --actor codex` 的 `standards.apply`；`POLICY-STANDARD-CONFLICT-UNTRACKED`/`STALE` 返回 `standards.conflict.refresh`；`POLICY-STANDARD-CONFLICT-UNRESOLVED` 返回 `standards.conflict.list`。只有 Draft 的 authored finding 才返回 `plan.apply`，Ready 中的 plan-scoped apply 会原子 reconcile 并使旧批准失效。

`plan create` 和 plan-scoped apply 都保存扫描 SHA-256、文件/目录/符号链接/字节计数以及稳定截断原因。截断扫描在 `agent context` 中返回 `scan_incomplete: true`，使 validate/finalize 保持阻塞且不会伪造完整扫描；`plan scan accept` 只接受该精确摘要。后续扫描摘要发生变化时，旧接受不会迁移到新的 digest。

目录 authoring 完全离线且仅接受数据文件。生成的 `catalog.toml` 延续既有 sync schema，补充的 `mino.team-catalog-manifest/v1` 保存 package、文件、目录树和大小身份，不允许可执行 payload。

### Agent API

| 命令 | 返回内容 |
|---|---|
| `mino agent context` | 完整项目、Git、project plan selection/alternatives、活动计划、扫描完整性、`executor_identity`、allowed/blocked actions、审批状态和规范 next argv。 |
| `mino agent next` | 聚焦 executor identity、project plan selection、当前计划、审批边界、blocked actions 和下一步。 |
| `mino agent capabilities` | 静态能力清单、稳定 executor identity，以及调用和 mutation 约束。 |

Agent 命令直接返回各自 schema，不套 `mino.result/v1`。缺少 JSON 或 no-input 模式时以 exit 5 失败。

<!-- doc-contract: next-actions-subset -->

每个 `next_actions[].id` 都是当前 `allowed_actions` 的成员。Approved Git Flow 下，Ready 计划或待自动提交的 Done task 若 binding 为 `missing`、`foreign_worktree`、`stale_branch`、`stale_head` 或 `not_repository`，下一步只返回精确 `git.bind` argv；刷新 context 且 binding 为 `current` 后，才分别返回 `exec.start` 或 `git.commit`。调用方不得在 returned argv 之外手工插入 bind、start、commit 或其他状态修改。

### 证据

| 命令 | 写入 | 核心契约 |
|---|---:|---|
| `mino evidence add` | 仅 evidence | 添加 File、GitDiff、Commit、Url、Log、Screenshot、ManualObservation 或 AcceptedException；Command evidence 只由 runner 创建。 |
| `mino evidence list` | 否 | 按递增 evidence ID 列出记录，可按 task/type 筛选。 |
| `mino evidence show` | 否 | 返回一条精确的不可变记录。 |

artifact path 必须留在项目内。修正证据会创建带 `supersedes` 的新记录，旧 record 和 blob 不会被改写。被修订失效的证据仍保留在历史中，但不能满足当前完成门槛。存在尚待 apply 的 amendment 时禁止添加证据。

Command evidence 还绑定实际被检查内容的 `WorkspaceFingerprint`：repository mode、HEAD、index tree、status entries、task/global scope、File Map snapshots 和 canonical digest。显式 File Map 的 ignored directory/glob 会绕过 `.gitignore` 重新枚举，但 `.git/**`、`.mino/**`、当前 projection、symlink/escape 和资源预算仍受保护。Git regular-file snapshot 同时保存 raw SHA-256 和按当前 attributes/index 语义计算的 `expected_git_entry { blob_oid, mode }`。criterion pass、task complete、自动/人工 commit、finish、review resolve 与 accept 都重新捕获原 scope；内容、对象类型、可执行位或适用 Git 身份变化会把检查持久化为 `Stale` 并要求重跑，不能用旧 Passed evidence 证明新字节。

### 执行、检查与调度说明

| 命令 | 写入/副作用 | 核心契约 |
|---|---|---|
| `mino exec start` | 计划 | 在批准后启动第一个 eligible Ready 任务，并要求此前所有必需任务提交已记录。 |
| `mino exec checkpoint` | 计划 | 为活动任务记录类型化 checkpoint；`--kind deviation` 是兼容入口，同时创建 Unclassified 的稳定 `D<n>`。 |
| `mino exec deviation record` | 计划 | 为活动任务创建带稳定 `D<n>`、classification、Open 状态和零个或多个规范化 `--path` 的偏差。 |
| `mino exec deviation list` | 否 | 返回 `mino.deviation-list/v1`，可按 task 筛选 Open 与全部历史终态。 |
| `mino exec deviation resolve` | 计划 | 用当前计划中未 stale、未 supersede 的任务 evidence 把 Open 偏差置为 Resolved。 |
| `mino exec deviation reject` | 计划，审批边界 | 用 decision reference 和 reason 把 Open 偏差置为 Rejected。 |
| `mino exec deviation supersede` | 计划 | 用已 Applied 的 Amendment 和 reason 把 Open 偏差置为 Superseded。 |
| `mino exec check run` | 计划、进程、证据 | 运行一项任务级或全局计划检查，保存 lease 和结果；普通终态保存 evidence，`capture_blocked` 只将 check 置为 Failed，不发布 evidence。 |
| `mino exec check monitor` | 计划、有限进程、证据 | 在次数、间隔和总 deadline 内重试一项已有检查，可使用安全取消文件。 |
| `mino exec schedule spec` | 无 | 输出摘要绑定、调度器中立的检查 handoff；不创建外部任务、不联网、不写 Mino 状态。 |
| `mino exec criterion pass` | 计划 | 把兼容的不可变证据绑定到一个活动验收条件。 |
| `mino exec complete` | 计划 | 在检查、证据、偏差、checkpoint 和 File Map 门槛后完成活动任务。 |
| `mino exec rework` | 计划 | 仅在必需全局检查失败后，用 `--task` 和非空 `--reason` 重新打开一个 Done 任务，重置其验收、检查、commit gate、任务基线和全部全局检查状态，同时保留历史 evidence。 |
| `mino exec block` | 计划 | 用非空且可恢复的原因阻塞 Ready 或 In Progress 计划。 |
| `mino exec resume` | 计划 | 恢复到记录的 Ready 或 In Progress 状态。 |
| `mino exec finish` | 计划 | 在所有任务、必需 commit gate、全局检查和完整 Final Outcome 完成后转入 Review。 |

只有 Open 偏差阻塞 task complete 和 exec finish；Resolved、Rejected 与 Superseded 保留全部审计字段但不再阻塞。旧状态中只有 Deviation checkpoint 时，读取会按 checkpoint 顺序生成确定性 `D<n>` 和 legacy checkpoint link；首次处置会把该记录持久化。Resolution evidence 必须属于同一计划和任务、未失效且未被替代；Superseded 必须引用已应用的 Amendment。

`exec finish`、`review resolve` 和 `review accept` 在写状态前执行同一个 Final Plan Delta gate。它合并 approved PlanBaseline 到当前文件系统的 delta 与 Git baseline HEAD 到 current HEAD 的 tree delta，因此已提交、未提交和非 Git 越界变化都可见。只有与 task File Map change kind 兼容的路径、`Resolved Minor` deviation 明确列出的精确路径以及 Mino-owned exclusions 被授权；其他路径以 exit 5 和稳定 `out_of_scope_paths` 阻止转换。

`plan approve` 捕获 project baseline，`exec start` 捕获 task baseline。task complete 比较的是当前 workspace 与 task-start baseline 的局部增量，而不是整个脏工作树；批准前未变化的 dirt、前一任务留下的未提交变化和非 Git 文件都按摘要区分。单个 fingerprint 文件最多 16 MiB，一次 capture 总计最多 256 MiB，超限会明确失败而不是退化为未跟踪变化。

相同 request ID 的 `exec check run` 可以安全精确重试。已有 terminal result 时返回 replay；原调用仍持有 run owner lock 时返回 exit 3 `revision_conflict`，消息标识 AlreadyRunning，并且不新增 evidence 或 plan revision；只有 owner lock 已释放且 lease 没有 result 时才恢复一次 `Interrupted` 终态。若 residual credential scan 命中，terminal result 为 `capture_blocked`，stdout/stderr 被清空，check 以无 evidence 的 Failed 状态结束，调用返回 policy violation；使用新 request ID 才能在修正检查输出后重新运行。

monitor 参数范围如下：

- `--max-attempts 1..=100`
- `--interval-milliseconds 1..=60000`
- `--deadline-milliseconds 1..=86400000`

总 deadline 必须在扣除所有可能间隔后，仍为每次尝试留下至少 1 ms。剩余预算在尝试间分配，每次检查最多五分钟，合并输出仍限制为 1 MiB。`--cancel-file` 必须是项目相对路径，其父目录已存在且留在项目内；Mino 只在尝试之间检查文件，不创建 watcher 或后台服务。

每次尝试使用确定派生的 child request ID，并通过正常的两次 check revision 与 Command evidence 流程。第一个终态被规范保存到 `.mino/plans/<plan-id>/monitors/<request-id>/summary.json`。完全相同的重试直接返回摘要；同一 request ID 的不同输入返回 exit 3。通过返回 exit 0，耗尽、deadline 或取消返回 exit 6，并保留所有已完成尝试的证据。

schedule spec 把当前 `--plan`、`--expect-revision` 和 `--check` 绑定到未来的完整 monitor argv。外层 handoff 还要求 RFC3339 trigger/expiry、有限 dispatch 次数与间隔、成功/停止/失败策略，以及安全的项目相对 result destination。trigger 到 expiry 最多 31 天，并必须覆盖最坏 monitor 和 dispatch retry 预算。

目标不能位于 `.mino/**`、`docs/plan/**`，不能逃逸项目，也不能经过符号链接或非目录父路径。返回的 `mino.scheduled-task-spec/v1` 明确包含 `external_creation_required: true` 与 `authorization_granted: false`；生成说明不等于授权调度器创建任务。

### Git 检查、绑定、分支、提交与 hooks

| 命令 | 写入/副作用 | 核心契约 |
|---|---|---|
| `mino git inspect` | 无 | 返回 repository、common directory、worktree、Git directory、index、HEAD、upstream、porcelain v2 和绑定事实。 |
| `mino git bind` | 仅 `.mino/active.json` | 把一个非 Done 计划绑定到当前 canonical worktree 与分支，或绑定精确 detached HEAD。 |
| `mino git branch propose` | 无 | 派生 `mino/<plan-id>`，交由 Git 校验，并报告 clean/base/source/ref blockers。 |
| `mino git branch create` | 本地分支、journal、binding | 要求 `--approval-ref`；可选 `--branch` 必须与提案完全一致。重新核对工作树和 base 后创建并切换。 |
| `mino git commit` | 精确 index、一个本地 commit、evidence、plan、journal | 只为第一个符合条件且 commit gate 待处理的 Done 任务执行。 |
| `mino git commit record-manual` | commit evidence、plan | 不修改 Git；要求当前分支 HEAD 的完整 commit ID、approval reference、revision 和 request ID，并验证 parent、消息、File Map、Commit Scope、无 clean filter 及检查期望的 commit-tree blob/mode。 |
| `mino git gate skip` | accepted-exception evidence、plan | 要求 approval reference、原因、revision 和 request ID；把 Pending/Blocked required gate 记录为可审计的 Skipped。 |
| `mino git hook propose` | 无 | 读取默认 hooks、ownership marker、模板/实际摘要和 custom hook 配置，生成稳定 proposal hash。 |
| `mino git hook status` | 无 | 返回同一组有界 ownership 与内容事实。 |
| `mino git hook install` | 仅 hook 文件，审批边界 | 要求当前 proposal hash 和 approval reference；只安装或修复 absent/Mino-owned 默认 hooks。 |
| `mino git hook run` | 无 | 读取 pre-commit 或 post-commit 的 staged/HEAD 与绑定事实，输出诊断和 next actions。 |

绑定状态只能是 `missing`、`current`、`foreign_worktree`、`stale_branch`、`stale_head` 或 `not_repository`。bind 只替换当前工作树 entry，不修改 HEAD、branch、ref、index、commit 或 `.mino/plan-selection.json`。foreign/stale 状态阻止需要当前 Git 身份的操作，但不会隐藏或切换 selected plan；项目方案始终由 project selection 独立决定。

branch create 是独立审批边界，计划批准和 Git Flow consent 不能替代它。Mino 先写 `.mino/git/branches/<plan-id>/intent.json`，再以禁用 hooks 的精确 `git switch -c` 操作 base HEAD，确认 post-state 后才写 binding 和 `completion.json`。精确重试能够区分未变化的失败、已创建待协调的分支和已完成操作。

git commit 不是新的会话审批边界；它只消费当前计划批准中明确的 Approved Git Flow 范围。前置检查会拒绝已有 staged path、index/worktree 混合内容、范围外路径、submodule、symlink、rename、clean filter、branch 或 parent drift。Mino 在 `git add -- <exact paths>` 前保存 raw digest 与 expected Git blob/mode 的 intent，随后验证 staged tree，使用计划中的单行消息运行正常 hooks，并再次用 commit tree 的逐路径 blob OID/mode（或 deletion absence）验证被检查内容后才写 Commit evidence、plan gate 和 completion。`text`、`eol` 与 `working-tree-encoding` 的内建转换由 expected entry 身份覆盖；自定义 clean filter 在自动和人工路径都被拒绝。失败现场会保留，不会自动 reset 或 unstage，也不会使用 `--no-verify`。

关闭 Git Flow 时，required commit gate 仍可在任务完成后通过人工路径闭环。`git commit record-manual` 只记录调用方已经创建的当前 HEAD；它不会运行 `git add` 或 `git commit`，并要求 task check fingerprint 的 HEAD 等于 commit parent、当前工作区 raw snapshot 仍新鲜、commit tree 的 expected Git entries 全部匹配。`git gate skip` 是独立审批边界，保存 AcceptedException evidence；任务顺序、finish 和 review 接受 Committed、Not Required 或已批准的 Skipped。

hooks 是可选建议层。Absent、Current 和 Mino-Owned-Drifted 可以进入幂等安装；用户 hooks、符号链接和 custom `core.hooksPath` 只返回手工集成说明。已安装脚本只调用 `mino git hook run`，即使 Mino 不可用或拒绝操作也会正常退出；运行时不修改 Git 或 Mino 状态。

Mino 的执行命令不会隐式修改 Git。Git 命令也不提供 push、merge、rebase、reset、amend、force-push、tag、branch deletion 或 worktree 创建/删除。

### 审阅与返工

| 命令 | 写入 | 核心契约 |
|---|---:|---|
| `mino review record` | 是 | 记录一项 Acceptance Defect、In-Scope Rework、Material Change 或 Follow-Up。 |
| `mino review rework` | 是 | 为 Acceptance Defect 重新打开任务，或从严格完整 YAML 实例化预留的 `R<n>` 任务。 |
| `mino review resolve` | 是 | 在当前任务、commit、全局检查、证据和偏差门槛均通过后解决一项返工。 |
| `mino review disposition` | 是，审批边界 | 用 `--decision accept-change|decline|defer-to-follow-up`、decision reference 和 reason 处置被阻塞的 Material Change。 |
| `mino review disposition revise` | 是，审批边界 | 仅在 Accept Change 关联的 amendment 终止且未应用后，用新的 decision reference 和 reason 改为 `decline` 或 `defer-to-follow-up`。 |
| `mino review accept` | 是，审批边界 | 要求 approval reference、全部反馈 resolved/deferred 和全部 live evidence 有效，随后进入 Done。 |

<!-- doc-contract: review-decision-revision -->

review item 使用连续 `REV-n`。In-Scope Rework 在 record 时预留单调递增的 `R<n>`，即使后续定义无效也不会释放。Acceptance Defect 保留之前的 committed gate，只接受 fresh evidence，并拒绝文件变更。Material Change 进入 review-owned Blocked，不能通过普通 resume 越过：`accept-change` 继续阻塞直至受保护 Material Amendment 应用，且 amendment 保存 `source_review_id`、Review item 保存 reciprocal link；`decline` 解决该项，`defer-to-follow-up` 把原 feedback 及来源 Review ID 同步到 Final Outcome 并成为非阻塞 Deferred。

Material disposition 是追加式 history。`review disposition revise` 只接受当前仍 Blocked 的原 Review item、原决定为 Accept Change、关联 amendment 为 `Rejected`/`Withdrawn`/`Cancelled` 且没有 replacement/applied change 的情形；新决定只能是 Decline 或 Defer。pending/applied amendment、错误 Review ID、第二次 revision 或再次 Accept Change 都被原子拒绝，旧 decision reference/reason/time 和 amendment terminal audit 不会被覆盖。普通 Follow-Up 同样保持 Deferred，不进入任务顺序，并同步来源关系。任何 review rework 或 Material apply 都会使旧 Final Outcome 失效，要求在新的最终检查通过后重写。

## 机器输出

### 通用成功 envelope

除 Agent API 外，JSON 模式会把命令 payload 展平到稳定的 `mino.result/v1`：

```json
{
  "kind": "mino.result/v1",
  "ok": true,
  "complete": false,
  "message": "Plan draft initialized.",
  "plan_id": "2026-07-25-example",
  "revision": 1,
  "missing": ["summary"],
  "next_actions": [
    {"id": "plan.summary.set", "argv": ["mino", "plan", "summary", "set", "..."]}
  ]
}
```

`ok` 表示命令是否成功，`complete` 表示请求的工作流是否仍有后续步骤；因此 `ok: true, complete: false` 是正常结果。`missing` 保存稳定位置或代码。`next_actions[].argv` 是包含 executable name 的完整参数数组，调用方不得把它重新拼成 shell string。

失败使用相同 kind：

```json
{
  "kind": "mino.result/v1",
  "ok": false,
  "complete": false,
  "message": "Plan revision is stale.",
  "error": {"code": "revision_conflict", "exit_code": 3},
  "missing": [],
  "next_actions": []
}
```

命令专用字段可以展平进入失败对象，但不得覆盖通用 keys。

### 稳定 schema 标识

| 标识 | 来源 |
|---|---|
| `mino.result/v1` | 所有非 Agent 成功或失败 envelope |
| `mino.agent-context/v1` | `agent context` |
| `mino.agent-next/v1` | `agent next` |
| `mino.agent-capabilities/v1` | `agent capabilities` |
| `mino.validation/v1` | 计划校验详情 |
| `mino.plan-review/v1` | revision-bound `plan review` payload |
| `mino.check-run/v1` | 持久化 check lease/result 的 `schema_version` |
| `mino.deviation-list/v1` | `exec deviation list` 的偏差生命周期列表 |
| `mino.monitor/v1` | monitor 终态摘要的 `monitor_kind` |
| `mino.scheduled-task-spec/v1` | 调度 handoff 的 `spec_kind` |
| `mino.plan-diff/v1` | 只读语义 diff 的 `diff_kind` |
| `mino.git-hook-status/v1` | hook status/proposal status |
| `mino.git-hook-proposal/v1` | hash-bound hook proposal |
| `mino.git-hook-install/v1` | approval-bound hook install result |
| `mino.git-hook-runtime/v1` | 只读 hook runtime observation |

计划聚合另有数值字段 `schema_version: 1`；protocol lock 分别绑定 protocol 和 renderer version。

## 退出码

| Exit | JSON code | 含义 | 调用方动作 |
|---:|---|---|---|
| 0 | N/A | 命令成功，即使 `complete` 为 false | 读取 payload 和 next actions。 |
| 2 | `incomplete_or_validation` | 输入不完整或确定性校验失败 | 只修正报告的字段。 |
| 3 | `revision_conflict` | revision 过期或 request UUID 输入冲突 | 刷新当前状态后再决定是否重试。 |
| 4 | `approval_required` | 需要显式用户批准 | 停止，不得替用户批准。 |
| 5 | `policy_violation` | 非法转换、不安全操作或策略拒绝 | 不得绕过门槛。 |
| 6 | `check_failed` | 计划检查未达到预期 | 保留证据，并按结果阻塞或修复。 |
| 7 | `environment_unavailable` | 必需文件、工具、服务、锁或环境不可用 | 报告依赖和最后状态。 |
| 8 | `drift_detected` | 规范状态、锁、不可变记录或受管字节不一致 | 保留字节并诊断或恢复。 |

Clap 参数错误也返回 2，但因为尚未进入 Mino dispatch，可能只输出 diagnostics 而不是 JSON envelope。

## 状态字符串

- Plan：`Draft`、`Ready`、`In Progress`、`Blocked`、`Review`、`Done`。
- Task：`Draft`、`Ready`、`In Progress`、`Blocked`、`Done`。
- Check：`Pending`、`Running`、`Passed`、`Failed`、`Blocked`、`Stale`；其中 `Stale` 表示先前 Passed evidence 的 workspace fingerprint 已不匹配。
- Criterion：`Pending`、`Passed`、`Failed`、`Accepted Exception`。
- Git Flow consent：`Pending`、`Approved`、`Disabled`。
- Commit gate：`Pending`、`Committed`、`Skipped`、`Not Required`、`Blocked`。
- Amendment classification：`Minor`、`Material`。
- Amendment state：`Proposed`、`Approval Required`、`Approved`、`Applied`、`Rejected`、`Withdrawn`、`Cancelled`。
- Deviation state：`Open`、`Resolved`、`Rejected`、`Superseded`。
- Material review disposition：`Accept Change`、`Decline`、`Defer to Follow-Up`。
- Project selection 使用独立的数值 `selection_revision`，不是 Plan lifecycle status。
- Active binding：`missing`、`current`、`foreign_worktree`、`stale_branch`、`stale_head`、`not_repository`。

只有命令清单中暴露的语义转换属于实现承诺；任何命令都不能接受调用方任意指定的 status value。

<!-- doc-contract: no-arbitrary-status-setter -->
