# 协议与旧工作流迁移

Mino 把“升级运行协议”“分析旧工作流文件”和“导入旧计划”拆成三条互不替代的路径。先根据目标选择路径，再执行对应命令：

| 目标 | 使用的命令 | 是否写入 |
|---|---|---|
| 核对当前项目和内嵌协议是否兼容 | `mino protocol status` | 否 |
| 执行已注册的协议状态转换 | `mino protocol migrate` | 仅存在 transform 时修改计划 |
| 了解旧 AGENTS、模板或执行指南如何映射 | `mino project migrate legacy` | 否 |
| 把一份旧 Markdown 计划变成新的 Draft | `mino project import legacy` | 创建独立计划 |
| 检查或解决 AGENTS 中的 Durable planning 双权威 | `mino project authority status/propose/decide/apply` | status/propose 只读；decide/apply 显式写入 |

旧文件中的“已批准”“已完成”“检查通过”等声明都不构成 Mino 信任。迁移只保留可验证的 authored intent，所有运行状态必须重新走当前协议。

## 当前协议身份

Mino `0.1.0` 把规划资源作为惰性字节内嵌，当前 manifest 身份如下：

| 字段 | 值 |
|---|---|
| Protocol version | `2026-05-11` |
| Protocol revision | `review-rework-git-flow-v1` |
| Plan schema | `1` |
| Renderer | `2` |
| `PLAN_TEMPLATE.md` SHA-256 | `73b55c3b64acc7e890464e180a4546b37c288984594f979e49dc117b7b634e9f` |
| `PLAN_EXECUTION.md` SHA-256 | `08076f7ecb2892bdb416c00b170af9d53a835bb5a7e7580f768059d318d1976a` |

初始化时，Mino 会验证 manifest 并写入对应的 `.mino/protocol.lock`。以下命令重新验证内嵌资源，再比较 lock format、protocol version/revision、plan schema 和 renderer：

```text
mino protocol status --format json --no-input
```

该命令只读，兼容性问题通过 `missing` 返回。内嵌 Markdown 只说明来源，不是运行时替代品；CLI 不可用时，不要复制模板到仓库来模拟 Mino。真正的约束来自编译后的 validator 与 state transition。

## 协议迁移

协议迁移必须绑定精确计划 revision 和 request UUID：

```text
mino protocol migrate \
  --plan <id> \
  --expect-revision <n> \
  --request-id <uuid> \
  --to <calendar-version> \
  --format json --no-input
```

当前 registry 没有旧版本 transform。已经使用 `2026-05-11/review-rework-git-flow-v1` 的计划会返回确定性的 `already_current`，且不写 revision、event、snapshot 或 projection。其他 source/target 组合返回 exit 5，并保留全部计划字节。

未来版本只有在显式注册并测试 transform 后才能修改旧计划；直接改写 protocol lock 不能构成升级。

## 版本升级与回滚

<!-- doc-contract: upgrade-and-rollback -->

binary、plugin 与项目本地协议是三个相关但独立的版本边界。升级一份 Mino 安装时按以下顺序处理：

1. 准备完整且经过验证的新 binary 或目标平台 plugin artifact。
2. 在项目中运行 `mino protocol status --format json --no-input`，不要假设安装新版本会自动修改 `.mino`。
3. 只有 status 返回已注册迁移时，才审阅并执行对应的精确 `protocol migrate`。
4. 运行 `mino project doctor`，再读取 `mino agent context`，确认 lock、projection、binding 和 active plan 一致。
5. 保留现有 plan、snapshot、event、evidence、review history、Git journal 与 standards cache；新增能力不会自动激活或授权自身。
6. 如果同时更换 Codex plugin，替换完整 target bundle，并重新运行 launcher 声明的 compatibility probes，不能混合新旧 Skill、launcher 和 binary。

回滚同样以完整 prior binary 或 plugin artifact 为单位。使用旧版本前先确认它仍支持项目当前 protocol lock；如果不兼容，应停止并恢复匹配的完整 artifact，而不是手工降级 `.mino` 文件。失败的 standards sync 会保留旧 cache，失败的 plugin build 不发布 target directory，Blocked 计划则通过记录的状态恢复。

## 增量状态兼容性

当前闭环能力保持 plan schema `1`，通过带默认值的 extension 和独立项目状态增加信息；读取旧状态不等于信任旧执行结果，也不会批量重写历史：

- **Workspace fingerprint 与 evidence**：没有 fingerprint 的旧 check lease/result 和 Command evidence 可以读取并审计；只有其余 invocation 字段完全相同的旧 lease 才能精确恢复。它们不能满足当前 freshness gate，必须重新运行检查后才能 criterion pass、complete、commit、finish 或 accept。
- **Plan/task baseline**：旧计划不会被合成一个“clean” baseline。新的 plan approval 与 task start 会从当时 workspace 捕获 baseline；需要 baseline 而状态中仍缺失时，操作明确失败，不会退回整个工作树猜测。
- **Project selection**：缺少 `.mino/plan-selection.json` 时使用只读的 selection revision 0。零个 live plan 返回空，一个 live plan 被虚拟选择；多个 live plan 全部作为 alternatives 返回，必须执行 approval-bound `plan select`。Git binding 不作为迁移 fallback。
- **Project scan**：缺少 scan extension 的旧计划保持可读；Mino 不伪造摘要。新 create 或 plan-scoped standards apply 会保存 scan digest、计数和截断原因。只有已保存且未接受的截断摘要阻塞 validate/finalize，接受记录不跨 digest 迁移。
- **Git readiness decision**：只有旧 observation 的计划可读取，但 setup/cleanup 会按 repository mode 和已保存 cleanliness 物化为兼容状态；受保护转换前仍必须运行 live `git readiness refresh`。缺少 repository 会产生 Pending setup，dirty tree 会产生 Pending cleanup，而不是自动禁用 Git Flow。`setup decide` 和 cleanup proposal/approval/record 都只写受审计的计划状态；Mino 不运行 `git init`、`git add` 或 pre-plan cleanup commit。
- **Planning authority**：存在普通 `AGENTS.md` 时初始化会创建 `.mino/authority.json`，绑定完整 source digest 与 fenced examples 以外的 active legacy clauses。缺少旧规则时不会阻塞；legacy 与 Mino workflow 同时 active 时必须显式 coexist、decline 或 guarded supersede。旧 decision 不跨任何 source byte 变化迁移。
- **Deviation 与 Final Outcome**：只有旧 Deviation checkpoint 时，会按历史顺序派生稳定 `D<n>` 与 legacy link；首次处置时持久化。旧 Review 可以通过 `plan outcome set` 补齐 Final Outcome，但不会自动进入 Done。
- **Actor identity**：既有 event 的 actor 原样保留。新的 Agent context/next/capabilities 宣告 `executor_identity: codex`，规范 revisioned mutation argv 显式传入 `--actor codex`；人工 CLI 省略 actor 时仍记录 `user`。

所有新受管读取限制都在解析前执行。旧 config/lock/plan/journal/evidence/projection 超过对应 1/4/8/16 MiB 上限时会返回 drift/corruption；升级不会截断、迁移或删除这些字节，应先保留现场并人工审计。

## 旧工作流分析

当仓库存在旧版 `AGENTS.md`、`PLAN_TEMPLATE.md` 或 `PLAN_EXECUTION.md` 时，先运行只读分析：

```text
mino project migrate legacy \
  --agents AGENTS.md \
  --template PLAN_TEMPLATE.md \
  --execution PLAN_EXECUTION.md \
  --format json --no-input
```

三个输入至少提供一个。每个文件必须是非空 UTF-8，大小不超过 1 MiB。报告会保留足够信息供人工判断：

- 精确路径、byte count 与 SHA-256；
- 按源顺序排列的 Markdown headings；
- 每部分的 `mapped`、`ambiguous` 或 `unsupported` disposition；
- duplicate、missing、ambiguous 和 unsupported findings；
- AGENTS 受管区块 diff，或模板/执行文档的惰性迁移建议；
- 固定的 `applied: false` 和空 `deleted_sources`。

分析不会编辑、重命名或删除任何旧文件。

## Durable planning authority 冲突

先读取只读状态和精确 rewrite proposal：

```text
mino project authority status --format json --no-input
mino project authority propose --format json --no-input
```

scanner 忽略 fenced Markdown examples，只检测 active Formal Plan Trigger、Pinned Gist/External Resource、Plan Review Gate 和 Plan Execution clauses。`status` 返回 authority revision、完整 `AGENTS.md` source digest、clause lines、Mino block 状态、decision、staleness 与 block reason；`propose` 返回完整 replacement digest 和唯一 section 的行范围，但不写任何内容。

有三种显式终局：

- `coexistence-approved`：保留 legacy text，但把它定义为非执行参考；Mino 独占 Durable workflow。
- `declined`：保留文本并阻止新的 Mino Durable plan。
- `superseded`：使用 `project authority apply --apply-rewrite`，仅把检测到的 `Planning Documents` section 替换为 Mino supersession 声明。

decide/apply 都要求 status/proposal 返回的 exact revision、source digest、唯一 request UUID、actor 和 approval reference；apply 还要求 exact replacement digest。apply 会先持久化 rewrite intent，再复用 recoverable integration transaction，最后才记录 superseded。精确重试可恢复任一中断点；pending intent 的 status 会从持久 audit 重建完整 recovery action，不要求猜测旧 revision 或摘要。terminal decision 绑定的 source 改变时，status 返回 canonical `project.init` refresh；刷新只把 detection 绑定到新摘要并回到 pending，不继承旧 approval。若 source bytes、对象类型或 proposal digest 改变，或者目标为 symlink、非普通文件、超过 1 MiB、并发被修改，操作拒绝覆盖并保留现场。rewrite 不触碰 section 之外的 Coding Standards、Git、MCP、语言规则或其他用户内容。

## 旧计划导入

导入适用于一份历史 managed Markdown 计划，并要求当前项目没有活动计划：

```text
mino project import legacy \
  --source legacy-plan.md \
  --name imported-change \
  --request-id 00000000-0000-0000-0000-000000000001 \
  --actor user \
  --format json --no-input
```

### 输入限制

源文件必须是普通文件、非空、UTF-8、无 NUL，最大 1 MiB。parser 识别 code fence，并支持有限的 front matter 与以下 authored 结构：

- Metadata、Summary、Context、Scope、Decisions；
- Approach、File Map、Interfaces、Edge Cases；
- 连续的 `T1..Tn` 任务；
- Git Flow declarations；
- Verification Plan。

报告为每个 mapping 给出源行、fragment、path、byte count 和 SHA-256，并返回稳定 warnings、目标 plan ID 与 revision。

### 保留与舍弃规则

只有能进入严格 `DraftPlanInput` 的定义会被导入。以下内容会被舍弃并产生 warning：

- duplicate、partial、placeholder 或 unknown 内容；
- 不连续或重复的任务 ID；
- absolute、traversal、backslash、`.mino/**` 或 `docs/plan/**` 路径；
- shell control syntax；
- 已知 shell 或 destructive executable 形式的检查。

历史 lifecycle、task/check/criterion status、commit result、review、approval 与 evidence 全部忽略。勾选过的 criterion 和 completed row 只会成为 Pending 定义，不携带证据。

导入通过普通 create/apply API 生成 revision 2 Draft，始终返回 `complete: false`。它不会 finalize、approve、execute 或 commit，也不会改动源文件。必须人工检查所有 mapping、warning、path、command、criterion 和 commit declaration，再走正常校验与批准流程。

相同 source bytes、name、actor 与 request UUID 的精确重试会重放两阶段结果。复用 UUID 但改变源摘要会被识别为 drift。

## 旧概念如何归属

| 旧工作流中的关注点 | 当前所有者 |
|---|---|
| durable plan trigger | 稳定 AGENTS 区块与 Skill description |
| 模板字段 | versioned `Plan` schema 与 deterministic renderer |
| 状态和执行顺序 | domain state machine 与 execution services |
| Ready 条件 | `plan validate` 与 `plan finalize` |
| 计划审阅与批准 | `plan review` 与 `plan approve` |
| 检查结果 | check-run journal 与 immutable evidence store |
| 被检查代码身份 | task/global `WorkspaceFingerprint` 与 freshness gate |
| 计划开始和任务局部变化 | plan/task baseline 与 task-local delta |
| checkpoint、block、resume | 对应的 `exec` semantic commands |
| 偏差接受、拒绝或由修订取代 | 带 `D<n>` 的 Deviation lifecycle |
| Git readiness 和 commit declaration | plan fields、File Map 与 Git policy |
| 人工 commit 或 commit exception | `git commit record-manual` 与 approval-bound `git gate skip` |
| Common/语言规则 | embedded standards 与 resolved checks |
| 多方案比较和活动选择 | project selection revision、`plan alternatives` 与 `plan select` |
| 固定外部模板 | embedded protocol bundle 与 protocol lock |
| 仓库专用工具路由 | 用户所有的 AGENTS 内容 |
| 发布、部署、通知 | 当前计划之外的手工或外部系统 |

## 推荐接入顺序

1. 按仓库原有策略备份或提交现有指令文件。
2. 运行 legacy analysis，逐条处理 ambiguous 和 unsupported finding。
3. 不带 apply flags 运行 `project init`，先审阅 Skill 与受管区块 proposal。
4. 接受 proposal 后，再显式应用 AGENTS 和 `.gitignore` 区块。
5. 运行 `project doctor` 与 `protocol status`；如返回 `legacy_planning_authority_conflict`，先审阅 authority status/proposal 并显式 decide 或 apply，直到没有 blocking finding。
6. 创建新计划，或导入一份受支持旧计划。
7. 如果 context 返回多个 alternatives，先审阅 diff，再用 `plan select` 明确选择；不要用 `git bind` 代替。
8. 读取 Git readiness：对缺少 repository 的计划显式决定初始化、无 Git 继续或等待人工设置；对 dirty tree 提交完整 cleanup proposal，并逐项取得批准。实际 Git 初始化和 cleanup commits 必须在 Mino 外按仓库授权完成，再用完整 OID record 和 readiness refresh 核验。
9. 人工复核导入 Draft、扫描摘要和 standards 后，再 validate、finalize 和 approve。
10. 重新运行所有需要 freshness 的检查；不要把旧 Passed evidence 当作升级证明。
11. 只有在独立审阅后才删除或简化旧文件；Mino 不执行这一步。

## 冲突与恢复

- 缺少 `<!-- mino-managed-skill:v1 -->` 的 Skill 属于用户，Mino 报告 `mino_skill_conflict` 并保留完整目录。
- marker-owned Skill 可以逐文件更新，目录内未知仓库文件会保留。
- 缺失区块只产生 proposal；valid-but-stale 区块可更新。duplicate、reversed、partial、non-UTF-8、symlink 或 non-file marker target 都会被拒绝。
- protocol migration、legacy analysis 和 import parse/digest 错误不会写计划状态。
- 导入若在 revision 1 创建后中断，可使用完全相同的导入请求补完 authored batch。
- Pending setup/cleanup 或 unsafe/File Map overlap 会把计划保持为可恢复 Blocked；完成外部 Git 工作后运行 revisioned readiness refresh。不要手工编辑 extension 或 projection，也不要把计划 approval 当作 pre-plan Git mutation 授权。
- Pending/stale planning authority、pending rewrite 或 declined decision 会阻止 Durable create。不要手工编辑 `.mino/authority.json` 或 transaction；用相同 digest-bound apply request 恢复中断，source 改变后重新检查并作新决定。
- 常规计划加载和 `project doctor` 会恢复 prepared transaction。不要手工删除 `.mino/**` 中的 transaction、snapshot 或 history 文件。

当前远程 Team Catalog package 只支持 sync/cache，不会被 recommend/apply 选入计划。普通完整 CI 仍只配置 Windows；多目标 artifact smoke 不能替代 Linux/macOS 全套验证。这两项是明确的后续边界，不应在迁移时解释为已经具备。
