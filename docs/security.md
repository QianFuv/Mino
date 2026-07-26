# 安全与操作边界

Mino 是本地协议引擎，不是授权服务，也不是操作系统 sandbox。它假设当前系统账户已经能够读写所选仓库，并能够启动批准计划中声明的工具；Mino 的职责是在这个账户权限内缩小动作范围、留下可审计记录，并拒绝协议之外的状态转换。

本文描述的是实现边界，而不是对恶意仓库代码、操作系统入侵或泄露凭据的完整防护。

## 信任模型

以下内容全部视为不可信输入：

- 仓库文件和配置；
- 用户需求与计划字段；
- 旧工作流和旧计划；
- 检查命令、stdout、stderr 与附件；
- 远程标准目录；
- 调用 Mino 的编码代理。

这些输入必须通过确定性的路径、大小、schema、摘要、revision 和状态策略。即使计划已经批准，也不能跳过其他边界。

### 审批不是通用权限

Mino 中的 approval reference 是可审计的用户声明，不是签名、身份认证或能力 token。各审批边界相互独立：

| 边界 | 必须明确批准的对象 | 不能替代它的状态 |
|---|---|---|
| `plan approve` | 当前 Ready revision 和 Git Flow consent 选择 | Draft 完整、检查通过 |
| `plan amend approve` | 当前待处理的 Material `C<n>` | 旧计划批准、Minor 分类 |
| `review accept` | 已解决反馈且证据仍有效的当前 Review revision | 计划批准、任务完成 |
| `plan archive` | 明确选择停用的计划、reason 与 approval reference | 创建了替代方案、计划 Done |
| `git branch create` | 当前确定性 branch proposal | 计划批准、Git Flow consent |
| `git hook install` | 当前 proposal hash 与目标 hooks | 计划批准、先前 proposal |
| `standards conflict resolve` | 当前 source fingerprint 中的具体候选、理由和 decision reference | 优先级默认值、旧冲突决定 |

项目批准不授权任意文件、网络、Git、部署、消息或远程系统操作。最终验收也不能从无失败检查、已解决反馈或会话语气中推断。

## 文件系统边界

Mino 会 canonicalize 项目根，并要求受管路径留在根目录内。需要项目所有权的字段拒绝 absolute path、parent traversal 和 symlink escape。

受管文件操作以 canonical 项目根打开 capability directory，随后只接受规范化的项目相对路径。创建目录前会逐级以 no-follow metadata 检查已有组件；symlink/junction、非目录祖先以及目标位置的异常文件类型都会被拒绝。目录创建后会重新验证，文件的读取、创建、替换、删除与目录同步也都相对于已打开的根句柄执行，因此 `.mino/**` 与 `docs/plan/**` 不能通过受管路径解析到项目外部。`project doctor` 只读报告 `managed_path_unsafe`，不会跟随或修复此类组件。

### 受管状态

- `.mino/plans/**` 保存规范计划、事务、快照、事件、运行和证据，禁止手工修改。
- `.mino/protocol.lock` 与 `.mino/standards.lock` 只能通过对应协议流程更新。
- `.mino/standards.local.toml` 是用户审阅的可选输入；引用来源必须是项目内普通文件并满足读取上限。
- `.mino/active.json` 只通过 `mino git bind` 修改；错误或过期身份只诊断，不静默修复。
- `.mino/git/branches/**` 与 `.mino/git/commits/**` 是不可变恢复日志，不是扩大 Git 权限的凭据。
- `docs/plan/*.md` 是摘要校验的投影；手工改动会产生 drift，并保留现场而不是被覆盖。

<!-- doc-contract: managed-state-no-manual-edit -->

计划事务、快照、事件、run journal、evidence record 与 blob 分别使用 create-new、guarded replacement、bounded lock、canonical bytes 和 digest check。`.gitignore` 对 `/.mino/` 与 `/docs/plan/` 的忽略不构成访问控制或加密；其中可能包含需求文本、路径、命令摘要、环境摘要和附件，分享前必须按敏感仓库数据处理。

Evidence 的 `records`/`blobs`、检查的 `runs`、monitor summary、active binding、Git branch/commit journal 以及 standards cache generation 均使用同一个 capability-rooted 文件系统入口。任一中间目录或最终目标被替换成 symlink/junction 或错误类型时，操作在发布外部 blob、summary、journal、cache 或 lock 之前失败；已有外部字节不会被读取为受管状态或被替换。

### 集成与导入

- Skill 和 marker block 更新会拒绝 symlink component、non-file target、unowned bytes 与 malformed/duplicate markers；合法更新只替换 owned bytes。
- legacy analysis 完全只读。
- legacy import 要求普通、非空、UTF-8、无 NUL、最大 1 MiB 的源文件，并保存路径、大小和 SHA-256 provenance。
- 导入时忽略 lifecycle、approval、result、commit、review 与 evidence 声明；unsafe path、shell control syntax 和已知 destructive executable 会被移除并产生 warning。
- fork 在创建目标前审计 source event/snapshot chain，任何缺失、损坏或 digest mismatch 都不会发布目标。
- archive 只追加 typed record，不删除、移动或重写计划历史。

## 进程执行边界

### 单次检查

`exec check run` 以计划中保存的 executable 和 argv 直接启动进程，不经过 shell。working directory 必须解析到项目内，child 只继承最小跨平台环境 allowlist，而不是完整 parent environment。

默认限制为五分钟和 1 MiB 合并 stdout/stderr；领域构造器允许的绝对上限为一小时和 16 MiB。超时、输出超限或 capture failure 时，Mino 使用 process group 或 Windows job object 终止 descendant processes。

spawn failure、unexpected exit、timeout、output limit、capture failure 与 interruption 都会形成持久终态。exit 6 不是“没有证据”：失败 evidence 会保留供审计，只是不能证明检查通过。

### 有限监控

`exec check monitor` 只能重试计划中已有的一项检查，且必须同时指定有限次数、间隔和 elapsed deadline。每次进程 timeout 从完整 retry budget 确定性计算，输出仍限制为 1 MiB。它在前台执行，不创建 daemon、scheduler、watcher 或无限轮询。

<!-- doc-contract: monitor-no-background-service -->

可选 cancellation file 必须是规范化的项目相对普通文件，其父目录已经存在并留在项目内。absolute path、traversal、symlink、directory 和 escaping parent 会在尝试前被拒绝。

终态 summary 有大小上限，绑定完整 request hash，并使用 no-clobber publication。失败、超时或取消仍保留所有已完成尝试的证据，并返回 exit 6。

### 调度说明

`exec schedule spec` 是惰性的本地读取操作。它会核对 plan、snapshot、projection、revision、check eligibility 及 trigger/expiry/retry bounds，但不会调用外部 scheduler、访问网络、写 result destination 或修改 plan、event、evidence、binding 和 projection。

<!-- doc-contract: schedule-no-external-mutation -->

输出明确声明 external creation required 且 authorization not granted。此前的计划批准或 Git Flow consent 都不是 scheduler consent。

result destination 必须是项目内的相对文件，父目录为现有普通目录。`.mino/**`、`docs/plan/**`、absolute path、traversal、symlink parent、missing parent 和已有 non-file 都会被拒绝。handoff 只包含显式 argv、有限 monitor 请求和结果策略，不包含 shell、daemon 或隐藏 scheduler API。

## 输出脱敏与证据

命令输出会先脱敏再计算摘要和持久化。默认规则替换形似 `api_key`、`token`、`secret`、`password` 和 `authorization` 的 key/value，并只记录 rule ID 与 count；secret-named allowlisted environment value 也会注册为 runtime literal redaction。

脱敏只是纵深防御，无法保证识别任意敏感信息。计划检查不应打印凭据；File、Log、Screenshot 等补充 evidence 也可能包含敏感字节。分享 `.mino` 或派生报告前必须人工审阅。

证据不可变：修正会创建通过 `supersedes` 关联的新记录。被 supersede 的记录不能通过当前 gate。`AcceptedException` 必须携带策略要求的 approval-compatible binding，不是通用绕过。Material amendment 会保留旧证据但标记 stale；pending amendment 阶段则拒绝新增证据，避免绑定到含糊输入。

## 网络边界

Mino 没有 telemetry，不会自动获取协议或标准更新。生产 CLI 唯一网络入口是用户显式运行：

```text
mino standards sync --all
```

sync 从 `.mino/config.toml` 读取 catalog URL。默认策略仅允许 HTTPS、不跟随 redirect、总耗时上限 30 秒、单个 catalog/document 上限 1 MiB、整个请求上限 16 MiB。只有全部 TOML、package identity 和 SHA-256 验证通过并完成新的 immutable cache generation 后，才会更新 `standards.lock`。

loopback HTTP 只存在于 library test policy，CLI 不会选择它。Evidence URL 与 legacy reference 只作为字符串保存，不会被 Mino 抓取。

## Git 与其他外部副作用

Git adapter 不经过 shell，禁用 terminal prompt，限制输出，并严格解析 machine-readable 结果。只读 root、repository、worktree、HEAD、index 与 status probe 还会禁用可选 Git locks。

### 只读检查与绑定

- `mino git inspect` 不写 Git 或 Mino 状态。
- `mino git bind` 只在 bounded lock 下原子替换 `.mino/active.json`，不修改 HEAD、branch、ref、index 或 commit。
- 活动计划必须匹配 canonical common directory 与 worktree。branch binding 要求相同 branch，detached binding 要求相同 HEAD；stale 或 foreign binding 不暴露活动计划。

### 本地分支

`mino git branch create` 是唯一 branch/ref creation path。它要求 approval reference，只接受确定性 proposal name，并重新核对 clean source、base HEAD 和 worktree identity。策略拒绝发生在 intent 与 Git mutation 之前。

通过策略后，Mino 先发布 immutable intent，再以 command-local `core.hooksPath` 禁用仓库 hooks，并在精确 base 上执行一次 `git switch -c`。只有确认 branch、HEAD 和 clean status 完全符合预期后，才写 active binding 与 completion。失败或中断会保留 intent 与观察状态供精确重试；Mino 不会 reset、delete 或 clean 来掩盖部分结果。

### 任务提交

`mino git commit` 是唯一 index/commit mutation path。它要求：

- 当前计划已批准且 Git Flow consent 为 Approved；
- 目标是第一个 commit gate 待处理的 Done 任务；
- 当前 same-worktree binding、branch 与 parent HEAD 精确匹配；
- task evidence 已满足；
- changed paths 同时落在 File Map 和 Commit Scope 内；
- 调用前 index 为空，不存在 mixed content 或 unsafe file kind。

通过 preflight 后，Mino 保存 bounded content snapshot 和 immutable intent，只对精确路径运行 `git add --`，记录 staged tree，再使用计划中的单行消息调用 `git commit`。正常 repository hooks 会执行，Mino 不使用 `--no-verify`。stdin 为空、terminal prompt 禁用，输出和运行时间均有限。

staging、hook 或 commit 失败会保留精确 staged state 与 journal，并把计划变成 Blocked。Mino 不会 reset、clean、checkout 或 unstage。`exec resume` 后的精确重试会核对 source/tree，并优先协调已经创建的 commit，避免重复提交。

### 建议型 hooks

hook install 只写已经检查过的默认 pre/post commit 路径；不会运行 `git config`，也不会 stage、commit、switch 或改 refs。用户 hooks、symlink、oversized file 和 custom `core.hooksPath` 都保留并转为手工集成。

运行时 hook 只读取 status、config、identity 与 binding，错误时仍正常退出；它不写 plan、event、evidence、active binding 或 hook 文件。

### 明确不提供的 Git 动作

Mino 不执行 push、merge、rebase、reset、amend、force-push、tag、branch deletion 或 worktree 创建/删除。`plan fork` 不调用 Git，也不继承 Git authorization；方案只能用 `plan diff` 比较，Mino 不提供 plan merge。

<!-- doc-contract: no-hidden-git-mutation -->

File Map 只接受规范化 exact path 和窄范围 `*`/`**` pattern。traversal、absolute path、malformed porcelain、duplicate path 和 out-of-scope change 都会阻止任务完成。

Mino 同样不会部署软件、发送消息、创建 ticket 或修改远程系统。这些动作需要其他工具和独立授权。

## 产品能力边界

以下限制适用于整个产品，而不是某个命令或发布版本：

<!-- doc-contract: no-llm-execution -->
<!-- doc-contract: no-daemon -->
<!-- doc-contract: no-cloud-control-plane -->
<!-- doc-contract: no-built-in-scheduler -->
<!-- doc-contract: no-auto-update -->
<!-- doc-contract: no-arbitrary-plugin-runtime -->
<!-- doc-contract: no-git-remote-or-destructive -->
<!-- doc-contract: no-plan-merge -->

- Mino 不执行 LLM 推理，不包含 prompt inference engine 或 autonomous agent loop。
- Mino 不创建 daemon、background worker、unbounded watcher 或隐藏 polling process。
- Mino 不提供 cloud control plane、telemetry、account service 或 Web UI。
- Mino 不内置 scheduler；schedule spec 只生成惰性数据，不创建外部任务。
- Mino 不自动更新、不发现远程 package、不下载替代 binary，也不发布 marketplace entry。
- Mino 不是 arbitrary plugin runtime，不加载远程 executable payload。
- Mino 不执行远程或破坏性 Git 操作，也不提供 plan merge。
- Mino 不允许通过手工修改 managed state、伪造 evidence 或任意设置 status 来声明成功。

## Agent 必须停止的情况

代理调用方需要在每个动作前读取 JSON/no-input context 中的 `approval_required`、`blocked_actions` 与 `next_actions`。遇到以下任一情况必须停止：

- `approval_required: true` 或 exit 4；
- 尚未获得对应 proposal/plan/review 的明确批准；
- exit 5 policy refusal 或 exit 8 drift/corruption；
- integration ownership malformed；
- 变更超出已批准 outcome、File Map、criterion 或 commit scope；
- review feedback 被分类为 Material Change；
- 存在尚待 approve/apply 的 amendment；
- 用户或仓库策略没有明确覆盖的 approval、exception、Git 或外部操作。

不得替用户批准，不得从会话语气或旧批准推断权限，不得在 Mino 不可用时伪造 plan/evidence state，也不得复制协议模板作为运行时 fallback。

<!-- doc-contract: no-protocol-template-fallback -->

没有通用 status setter：Review 到 Done 只能通过 `review accept`，返工必须先分类记录再执行 `review rework` 与 `review resolve`，停用计划只能通过 approval-bound `plan archive`。

## 恢复步骤

1. 保留当前全部字节和命令输出，不先清理现场。
2. 运行 `project doctor`、`protocol status` 和相关只读 show/list 命令。
3. revision conflict 后重新读取 Agent context。
4. 只有确实要重放同一操作时，才使用完全相同的 request UUID 与 argv。
5. 使用返回的规范 remediation；不要手工删除 lock、journal、snapshot、evidence 或 projection。
6. 如果 corruption 或 marker conflict 仍存在，从经审阅的备份恢复、手工协调，或联系维护者。
