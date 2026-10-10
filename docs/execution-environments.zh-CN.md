# 执行环境（内部接口）

面向持久化用户执行身份的内部控制面切片（issue 737/742 P3）。仅为增量式
基础设施：不涉及 Agent 端点、原生工作区挂载、跨 manager 故障转移或生产
迁移。异步意图确认绝不等于运行或停止确认。

## 认证

所有调用均需应用 bearer 令牌加 `X-DF-External-User-Id`（沿用现有
`application_actor` 约定，与 lease 路由一致）。应用身份完全来自
bearer；请求体中的 `application_id` 会被拒绝。该头仅用于认证：
环境身份来自 JSON 请求体。

## 接口

### `POST /api/internal/execution-environments/ensure`

针对 `(application_id, external_user_id)` 的幂等 ensure。

请求（`EnsureExecutionEnvironmentRequest`，未知字段直接拒绝）：

```json
{"external_user_id": "user-123", "runtime_kind": "opencode"}
```

- `external_user_id`：不透明、非空，最大 128。
- `runtime_kind`：不透明且与 agent 无关，DF 永不据此分支，非空，
  最大 64。

行为：

- 首次调用插入一行：全新 UUID `environment_id`、`generation` 0、
  `state` `starting`、`state_volume_ref` 为 `df-env-<uuid-simple>`
 （惰性命名，不实际分配存储）。
- 重复或并发调用返回相同 `environment_id`、`state_volume_ref` 和
  `generation`，不做修改。
- 同一身份使用不同 `runtime_kind` 返回 `409`，且不修改原行。
- 挂载路径、卷引用、容器地址与 `application_id` 均不可传入
 （`400` 拒绝）。

响应 `200`：`ExecutionEnvironment`（`environment_id`、
`application_id`、`external_user_id`、`runtime_kind`、`generation`、
`state`、`state_volume_ref` 及时间戳）。

### `POST /api/internal/execution-environments/{environment_id}/invalidate`

面向属主的原子 fencing 步骤。

请求（`InvalidateExecutionEnvironmentRequest`，未知字段直接拒绝）：

```json
{"expected_generation": 0, "reason": "replace"}
```

- `expected_generation`：`>= 0`，与存储值精确比较。
- `reason`：可选诊断信息，最大 512。

行为：

- 完全匹配时：`generation = generation + 1`、`state = 'stopping'`，
  返回 `200` 与更新后行。
- 不匹配时：返回 `409`，不做任何修改。
- 未知或跨应用 `environment_id`：返回 `404`，不做修改。
- Invalidate 不代表已停止写入者：本阶段无写入者、无路由。
  `GET /api/internal/application/capabilities` 保持
  `execution_environments_ready: false`。

## 状态与 generation

- `state` 受 CHECK 约束：`absent`/`starting`/`ready`/`stopping`/
  `replaced`/`unreachable`/`uncertain`。新建为 `starting`；成功 invalidate
  后为 `stopping`。
- `generation` 单调递增且 `>= 0`。Generation 与 `state_volume_ref`
  只是引用，不是安全凭证。

## 异步生命周期

上述元数据 `ensure`/`invalidate` 路由语义不变。另有四个已认证路由以
异步方式接纳生命周期意图，并返回公开生命周期描述符
（`EnvironmentOperationDescriptor`：`environment_id`、`generation`、
`state`，以及可选的 `operation`（`job_id`/`kind`/`state`））。
不暴露状态卷引用、操作令牌、manager id、宿主机端点或任何机密。

所有调用先从 bearer 解析应用身份，再执行一个短暂的属主事务，对
execution_environments 行加 `FOR UPDATE`。请求只携带 fenced 输入与有界
`operation_token`（`1..=128`，`deny_unknown_fields`）；绝不接受
`application_id`、manager、镜像、路径或端点。

### `POST /api/internal/execution-environments/{environment_id}/provision`

接纳一次 ensure 意图。要求 provision 候选为空且无活跃冲突任务；
仅在首次 provision 时绑定一个 enabled+online+近期心跳的 manager
（不可变，永不重绑），记录候选 generation，状态置 `starting`，并向绑定的
manager 入队 `ensure`。相同令牌重放返回原操作；候选已存在或令牌意图冲突
则拒绝。无可用 manager 时返回 `503` 且不绑定任何 manager。

### `POST /api/internal/execution-environments/{environment_id}/stop`

无候选时拒绝 `not_provisioned`（绝不伪造停止凭证）。在预期 generation 上的
首次 stop 只将授权 generation 递增一次，保留候选，清除此前凭证，状态置
`stopping`，并向绑定 manager 入队针对该候选的 `stop`。在 `uncertain` stop
之后的恢复 stop 会再次接纳，但不再递增、不改变候选、不重绑 manager；
相同令牌重放不会再次递增。任何状态决策前，超期的 `pending`/`claimed`
任务先失败关闭为 `uncertain`；manager 离线绝不产生停止凭证。

### `GET /api/internal/execution-environments/{environment_id}/status`

只对已持久化证据做对账（绝不派发）。它校验每个任务的类型化结果、任务的
assigned manager 是否等于环境绑定、目标 generation 是否等于候选。匹配的
成功 ensure 仅当授权 generation 等于候选且无未决 stop 时才置 `ready`；
匹配的已确认 stop（`stop_confirmed`、`confirmed=true`、观察状态为
`absent`/`exited`/`dead`）记录凭证任务与已停止 generation，置 `replaced`，
并将匹配的过期 `ensure` 行标记为 `superseded`（显式停止凭证结果，绝不为
ensure 成功）；未确认的 stop 将 `stopping` 降级为 `uncertain`。
重复对账幂等。

### `POST /api/internal/execution-environments/{environment_id}/replace`

接纳下一次 ensure。预期授权 generation、候选与停止凭证任务必须全部匹配；
凭证行必须仍是原绑定 manager 的、已确认的 succeeded stop；且不得存在活跃
stop 或未决 ensure 阻塞项。它把候选推进到当前授权 generation（必须大于已
停止候选），消费凭证，状态置 `starting`，并向绑定 manager 入队 `ensure`。
过期或已消费的凭证一律拒绝；manager 绑定永不重绑。

## 生命周期持久化

一个前向迁移（`migrations/*_execution_environment_lifecycle_proof.sql`）
新增 `assigned_manager_id`（可空，`RESTRICT`）、`container_generation`
（可空，`>= 0`；NULL 表示无候选）、`stop_proof_job_id`、
`stop_proof_generation`，以及环境 `uncertain` 状态。同时新增任务
`superseded` 终态（要求 `terminal_at`），并把 stop 活跃索引收窄为
`pending`/`claimed`，使先前的 `uncertain` stop 仍被记录但不再阻塞恢复
stop；ensure 活跃索引在所有 generation 上保持
`pending`/`claimed`/`uncertain`。不修改任何既有迁移。

## 约定说明

- OpenAPI 由注释生成；修改后执行
  `cargo run -p desk-foreman --bin export-openapi -- openapi.json`，
  并确认第二次导出无 diff。
- 生命周期静态 SQL 位于 `src/sql/execution_environments/lifecycle/`，
  以 `query_file!`/`query_file_as!` 加载；`.sqlx` 元数据在隔离且已初始化
  的 PostgreSQL 上通过
  `cargo sqlx prepare --workspace -- --all-targets` 重新生成。
- `GET /api/internal/application/capabilities` 保持
  `execution_environments_ready: false`；没有任何路由启用环境创建或声称
  agent 就绪。
- DF 保持 agent 中立：OpenAPI 不含 OpenCode/ACP/prompt/AG-UI 类型。
  这些路由不可达任何 Docker 或分配调用。
