# 持久化环境操作记录与派发（内部）

环境操作的内部持久化与指定 manager 派发切片（issue 741/742）。它持久化操作
意图、认领与结果，通过现有已认证的 runner-manager 传输把待处理记录派发给
指定 manager，并阻止执行环境属主进入旧的工作区路径。此处不含 manager 停止
屏障、生命周期编排或生产启用。

## 存储

`execution_environment_jobs` 每个操作意图一行：某一执行环境某个确切
generation 的 `ensure` 或 `stop`。

| 列 | 含义 |
| --- | --- |
| `job_id` | UUID 主键，稳定的线上身份 |
| `token` | 调用方选择的不透明重放键，全局唯一 |
| `environment_id`、`application_id` | 属主对，由复合外键保证一致 |
| `assigned_manager_id` | 入队时固定、不可更改的 manager（`RESTRICT`） |
| `job_kind` | 受约束的 `ensure` 或 `stop` |
| `target_generation` | `>= 0` |
| `state` | 受约束的 `pending`/`claimed`/`succeeded`/`failed`/`uncertain` |
| `claimed_at`、`deadline_at`、`terminal_at` | 生命周期时间戳 |
| `result` | 类型化 JSON 结果，绝不含宿主路径或机密 |
| `error_summary` | 有界、已脱敏的诊断字符串 |

两个部分唯一索引保证每个环境在**所有** generation 上最多各有一个活跃
`ensure` 与一个活跃 `stop`。活跃 `ensure` 与活跃 `stop` 可以并存：它们是
持久记录，而非可并发的运行操作。

## 状态机

- `pending` -> 由指定 manager 在截止时间前认领为 `claimed`，恰好一次。
- `claimed` -> 由指定的认领者在截止时间前完成为 `succeeded`/`failed`。
- 超期的 `pending`/`claimed` -> `uncertain`，失败关闭。`uncertain` 为终态，
  不再可认领，且始终是活跃阻塞项；没有任何 API 会清除或重试它。

## 操作

- `enqueue_execution_environment_job`：在 SQL 内校验属主对与已启用 manager
  后插入。相同 token 的相同意图重放原行；同一属主、不同意图返回
  `TokenConflict`；同类型活跃操作返回 `ActiveConflict`，且不暴露既有行；
  跨属主对或缺失/停用的 manager 返回 `EnvironmentNotFound`。
- `find_execution_environment_job_by_token`、
  `find_active_execution_environment_job`：按属主作用域的读取；活跃查询返回
  pending/claimed/uncertain 阻塞项。
- `claim_execution_environment_job`：单条原子 CAS；并发调用恰好一个胜出，
  错误 manager 与超期任务永不认领。
- `complete_execution_environment_job`：对 job id、属主对、token、指定认领者、
  `claimed` 状态与未超期截止时间的原子 CAS。相同的终态重放返回已记录结果；
  冲突重放被拒绝；截止时间之后的完成永不成功。
- `expire_execution_environment_jobs`：将某个属主环境下超期的 `pending` 与
  `claimed` 原子迁移为 `uncertain`。
- `claim_next_assigned_manager_environment_job`：仅 manager 可用的认领，以
  单条原子更新加 `FOR UPDATE SKIP LOCKED` 取走指派给已认证 manager 的最旧
  `pending` 行；超期、已认领与 `uncertain` 行永不可认领。
- `find_execution_environment_job_by_id` 与
  `complete_assigned_manager_environment_job`：仅 manager 可用的按稳定 UUID
  受信查找与完成，仅供已认证的 runner-manager 结果路径使用。

## 指定 manager 派发

持久任务使用一套保留的不透明线上 id：`envjob:<UUID>`，因此与旧 broker 的
纯 UUID id 永不冲突：

- `GET /api/internal/runner-manager/jobs/next` 先为已认证 manager 尝试持久
  认领。认领成功的行以既有 `RunnerJob` 返回，kind 为
  `ensure_execution_environment` / `stop_execution_environment`，payload 仅含
  owner，正是 runner-manager worker 已能反序列化的形状。无可认领持久任务时，
  路由保持未改动的旧 broker 行为。
- `POST /api/internal/runner-manager/jobs/result` 将保留的 `envjob:` id 独占
  路由到持久处理；畸形的带前缀 id 被拒绝，绝不回退到旧 broker。纯 UUID 结果
  仍路由到旧 broker。

行是唯一权威，绝不信任客户端：解析 id 后处理器查找该行，校验已认证 manager
等于 `assigned_manager_id`，并从行派生 application、environment、token 与
owner。上报的 ack 必须能反序列化为既有 `EnsureExecutionEnvironmentAck` /
`StopExecutionEnvironmentAck`，匹配确切的 `RunnerOwner` 与派生容器名，并满足
其类型的成功规则——`ensure` 仅在 `running` 时成功；`stop` 仅在
`confirmed=true` 且观察状态为 `absent`/`exited`/`dead` 时成功（`removed` 与
任意字符串均不接受）。终态结果：

- 一致成功 -> `succeeded`，带类型化结果（`ensure_running`、`stop_confirmed`）；
- 有效但未确认的 stop，或已诊断的失败 -> `uncertain`，带类型化结果
  （`stop_unconfirmed`、`ensure_uncertain`）与固定脱敏诊断；manager 的原始
  错误文本永不持久化；
- 畸形或不一致的 ack、错误 manager、未知行都不改动任何数据，并在存储之前
  被拒绝。

相同终态重放返回已记录结果；冲突重放被拒绝；迟到的成功无法清除
`uncertain`。既有完成 CAS 维持该边界（含未超期 `claimed` 谓词），因此超期的
传输超时保持未回收，而非被静默清除。

## 租户隔离与旧路径排除

属主关系在 SQL 中强制，而非信任调用方：每个操作同时限定
`application_id` 与 `environment_id`，插入时从 `execution_environments` 按
属主应用筛选，复合外键（由 `execution_environments (environment_id,
application_id)` 上的新增附加唯一约束支撑）拒绝跨属主配对。任何情况下都不会
返回其他应用的行，包括 token 全局碰撞时。

`DockerRunnerBackend::ensure_runner` 会在构造挂载或执行任何 Docker 命令之前
拒绝执行环境属主，因此环境属主无法进入旧的工作区/用户路径。现有内部用户与
工作区行为保持不变。

## 约定说明

- 静态 SQL 位于 `src/sql/execution_environment_jobs/`，以
  `query_file!`/`query_file_as!` 加载；不存在运行时静态 SQL 字符串。`.sqlx`
  元数据在隔离且已初始化的 PostgreSQL 上通过
  `cargo sqlx prepare --workspace -- --all-targets` 重新生成。
- `GET /api/internal/application/capabilities` 保持
  `execution_environments_ready: false`，且两者都不启用环境创建：派发层只在
  已指派的 manager 与持久记录之间传输。
- 该存储只提供持久操作记录；不提供分布式进程 fencing，不能声明 manager 缺失，
  也永不丢弃迟到的运行任务、重置 `uncertain` 或释放卷。
