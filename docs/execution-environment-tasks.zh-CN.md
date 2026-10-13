# 持久化原生任务意图

不可变输出选集与下载见[已保留任务的显式选定文件导出](execution-environment-task-exports.zh-CN.md)。

应用为已授权的工作区绑定登记一个持久化的任务意图。任务状态从登记与准备，推进到
围栏保护的启动和任务级停止。控制面为原始 Manager 记录持久化作业；只有匹配的类型化
启动确认才能将任务标记为 `running`。停止确认必须同时证明环境已停止且准备写入者已静默，
并且应用显式调用 release 后才会释放工作区。

```
应用 ──HTTPS──▶ 控制面 ──▶ 持久化任务行
                  │
                  └─ 按属主校验的围栏；每个环境与每个工作区同时只有一个活跃任务
```

任务 UUID 即属主。它**不是**授权代次，也不是已挂载的容器代次：代次只是围栏，
任务活跃期间代次可以推进，因此处于 `uncertain` 的任务会跨所有代次持续阻挡新的
意图，直到运维人员显式解决。

## 端点

所有路由都从 Bearer 令牌解析应用身份（绝不从请求体解析），并拒绝未知字段。

### `POST /api/internal/execution-environments/{environment_id}/tasks/admit`

登记一个任务意图；当以相同 operation token 重放同一意图时，幂等地返回既有任务。

| 字段 | 含义 |
| --- | --- |
| `task_id` | 调用方已知的稳定任务 UUID。它是标识符，不是凭据。 |
| `workspace_binding_id` | 要绑定的已授权工作区绑定。 |
| `subject_external_user_id` | 必须等于该环境自身的 subject。 |
| `expected_generation` | 该环境当前的授权代次。 |
| `operation_token` | 幂等键，长度上限 128 字节。 |

### `GET /api/internal/execution-environments/{environment_id}/tasks/{task_id}`

读取一个归属于本应用的任务、当前状态、候选代次，以及可选的
`start_job_id` / `stop_job_id` 锚点。响应还包含 `workspace_directory`，即该绑定在容器内的
检出位置，详见[原生任务工作区目录](native-task-workspace-directory.zh-CN.md)。

### `POST /api/internal/execution-environments/{environment_id}/tasks/{task_id}/cancel`

按调用方当前代次取消一个意图。可证明从未启动的任务——没有候选代次、也没有任何
持久化作业引用它——会被作废并释放其环境与工作区。其余任务会被标记为
`uncertain` 并继续阻挡。

### `POST /api/internal/execution-environments/{environment_id}/tasks/{task_id}/release`

依据任务自身已确认的停止证明释放一个已停止任务。每个字段都是围栏：证明必须属于
该任务、来自该任务最初的 Manager、针对其候选代次，并记录正向环境确认及
`preparation_quiescent: true`。停止作业绝不会自动释放任务。

### `POST /api/internal/execution-environments/{environment_id}/tasks/{task_id}/seed`

为已登记任务封存一份不可变的初始文件快照（由应用授权），或幂等地返回既有且内容相同的
快照。

| 字段 | 含义 |
| --- | --- |
| `expected_generation` | 该环境当前的授权代次。 |
| `files` | 已授权的文件：`path`（相对 POSIX 路径）与 `content`（UTF-8）。 |

响应只携带身份信息——`task_id`、`manifest_sha256`、`file_count`、`total_bytes`
——绝不含路径、凭据或宿主细节。

### `GET /api/internal/execution-environments/{environment_id}/tasks/{task_id}/seed`

按规范路径顺序返回带内容的已封存快照。快照属于制品历史，因此在任何任务状态下都会
原样返回，包括 `released` 与 `uncertain`。

### 响应结构

任务描述只携带身份、绑定、代次与状态，绝不含宿主路径、状态卷、镜像、端口、
凭据、Manager 端点或 subject。

| 状态码 | 含义 |
| --- | --- |
| 200 | 任务，处于该操作产生或发现的状态。 |
| 400 | operation token 缺失或超长。 |
| 401 | 缺少应用 Bearer。 |
| 404 | 属于其他应用的、或未知的环境、绑定、任务或证明。外来资源与不存在的资源不可区分。 |
| 409 | 围栏不匹配、subject 不匹配、绑定不合格、存在遗留写入属主、存在未解决阻塞、token 冲突或活跃任务冲突。 |
| 413 | 种子请求体、条目数或内容大小超过允许上限。 |
| 503 | 原始 Runner Manager 不可用于任务登记、启动或停止。 |

## 登记规则

只有同时满足以下条件时才会登记任务。先锁环境行、再锁工作区绑定行，并按此顺序
持有直到意图提交。

- 环境属于调用方应用，且其代次与调用方的围栏一致。
- subject 与该环境自身的 subject 一致。资源绑定上固定的 external_user 标记
  **绝不**与之比较：授权来自已认证应用的委托，该标记仅用于保持共享行唯一。
- 绑定是活跃的 `code_project` 资源工作区，且其生命周期允许登记。
- 不存在遗留写入租约属主，**即使是已过期的也不例外**。过期只是清理定时器，
  并不能证明没有已派发的进程仍在写入。
- 该环境不存在未解决的 ensure 或 stop 操作。
- 该环境与绑定都尚未持有活跃任务。

任务绑定的 Manager 是该环境不可变的绑定，或首次 provision 所用的同一套内部选择。
即使在 Manager 离线后，任务也绝不会改绑到替代 Manager。

## 排他性

两个 partial unique 索引与相应的登记守卫共同保证：每个环境、每个工作区绑定在
`admitted`、`preparing`、`prepared`、`starting`、`running`、`stopping` 和
`uncertain` 状态下最多只有一个活跃任务。`released` 任务会释放两者。

## 遗留围栏写入者

持久化任务与遗留工作区写入者会以互不相通的围栏写入同一工作区，因此只要存在活跃
任务，所有遗留路径都会拒绝：

- 围栏写入准入（`lock_workspace_binding_for_write`）；
- 遗留写入租约的获取、释放与强制接管；
- 管理端的 archive、restore 与 reset；
- 经由资源工作区路由的 git/sync；
- 归档工作区清理（janitor）扫描——它跳过而非删除。

上述每一条都会在**已持有自己的绑定行锁之后**、在同一事务内重新读取原生活跃守卫。
在加锁之前求值的检查读到的是可能早于「等待期间被登记的任务」的快照——这正是该
守卫要封闭的交错。任务登记会获取同一把锁，因此要么任务先提交（遗留路径即可看到
它），要么任务等待（并看到该遗留转换）。

只要存在活跃任务锚点，环境元数据 invalidate 以及遗留 provision、replace、stop 路由也会拒绝。
Status 可以读取当前环境/作业描述，但绝不会通过遗留生命周期路径协调任务级 start/stop 作业。

不存在原生任务时，所有遗留路径的行为与之前完全一致。

## 不可变的任务输入快照

已登记任务可以携带一份已封存快照，内容为应用为其授权的文件。快照不可变：没有覆盖或
删除路径，以不同内容再次封存即为冲突。

### 上限

| 限制 | 值 |
| --- | --- |
| 请求体（传输层上限，解码之前） | 16 MiB |
| 条目数 | 4096 |
| 单个路径（UTF-8 字节） | 512 |
| 单个内容（UTF-8 字节） | 1 MiB |
| 内容总量（UTF-8 字节） | 8 MiB |

空文件列表表示合法的空工作区。

### 路径规则

路径必须是规范化的相对 POSIX 路径。以下一律拒绝：绝对路径、Windows 盘符前缀、反斜杠、
NUL 及其他控制字符、空的 `.`/`..` 分量、结尾斜杠、重复路径、既是文件又是另一路径的目录
祖先、以及任意大小写形式的 `.git` 分量。超过 512 字节的路径与其他所有形态规则均为
`400`；请求体、条目数以及内容/总量大小均为 `413`。含 NUL 的内容在任何 SQL 之前即被
拒绝，因为 Postgres `TEXT` 无法存储它。

### 清单

摘要是仅针对文件字节的固定版本化编码，因此与 JSON 空白、请求顺序、任务 ID 和代次
无关——这正是后续阶段能将该内容与规范项目做 CAS 的前提：

```
"df-native-task-seed-v1" NUL
按路径字典序对每个文件：
    路径字节长度  u64 大端
    路径字节
    内容字节长度  u64 大端
    内容字节
```

结果是上述字节的小写十六进制 SHA-256。由于排除了任务 ID 与代次，相同内容在任何位置
都会得到相同哈希。

### 封存规则

只有当任务仍是该环境活跃的 `admitted` 意图、没有候选容器代次、也没有任何持久化作业
引用时，才会写入快照；环境行锁会串行化封存，因此对同一任务的并发封存只会留下唯一一份
持久输入。封存事务依次锁定环境、任务、绑定——与其他任务变更完全相同的顺序。

## 原始 Manager 准备

带有已封存种子的已登记任务可以被**准备**：控制面记录并向该任务的**原始** Manager
派发一个持久化作业，由 Manager 将种子物化到私有检出目录并返回类型化证明。准备操作不会
启动容器，也不会写入规范根目录。

### `POST /api/internal/execution-environments/{environment_id}/tasks/{task_id}/prepare`

调度（或幂等重放）一次准备。

| 字段 | 含义 |
| --- | --- |
| `expected_generation` | 该环境当前的授权代次。 |
| `operation_token` | 幂等键，长度上限 128 字节。 |

响应为持久化描述：`task_id`、`task_state`、`job_id`、`job_state`、
`target_generation`、`active`、`released`，不含宿主路径、Manager 端点或种子内容。

### `GET /api/internal/execution-environments/{environment_id}/tasks/{task_id}/prepare`

返回归属本应用的同一描述。不存在或属于他人一律 `404`。

### `GET /api/internal/runner-managers/{manager_id}/environment-jobs/{job_id}/task-seed`

Manager 用自己的 Bearer 获取分配给它的作业的已封存种子字节。仅当已认证 Manager 就是该
作业的分配 Manager、作业是活跃的 `prepare_native_task` 认领、且任务仍在该作业所针对的
代次上处于 `preparing` 时才返回；其余一律 `404`。

### 准备规则

调度依次锁定环境、任务、绑定——与其他任务变更完全相同的顺序——并要求全部满足：

- 任务仍是该环境活跃的 `admitted` 意图，且没有候选容器代次；
- 任务的绑定是活跃的 code project 工作区；
- 已存在已封存种子（没有可物化的内容是拒绝的）；
- 不存在未解决的 `ensure`/`stop` 操作；
- 任务的**原始** Manager 仍然启用、在线且近期可见——准备绝不会切换到其他 Manager。

任务在一次提交中转为 `preparing` 并链接其作业。相同 token 重放返回既有作业的当前状态；
不同 token 为冲突。成功且完全匹配的 ack 会把任务标记为 `prepared`——绝不会把环境标记为
`ready`，也绝不会设置容器代次。失败、超时或不匹配的准备会把关联任务标记为 `uncertain`，
保留其独占的环境与工作区锚点；它绝不会被重新派发或改绑，且取消之后的迟到 ack 绝不会
重新标记已终结的任务。

`prepared` 是活跃任务状态：与 `preparing` 一样，它独占环境与工作区绑定，因此不会有新的
任务或遗留写入者占用它们。

## 原生任务启动与停止

### `POST /api/internal/execution-environments/{environment_id}/tasks/{task_id}/start`

为已准备任务调度或重放一次启动。严格请求只包含 `expected_generation` 与不透明的
`operation_token`。只有当任务仍是环境活跃的 `prepared` 任务、其关联准备作业已针对封存种子
成功、任务/环境围栏匹配、任何旧候选都已有匹配的正向停止证明，并且原始 Manager 仍启用、
在线且近期可见时才会接受。绝不会切换到其他 Manager。

响应包含环境与任务状态、作业身份及 `native_task_id`。调度会在一个事务中把任务和环境设为
`starting`、记录候选并关联作业；这只是候选，不是运行确认。Manager 请求仅包含已存储的任务
身份（任务 ID、绑定、种子清单与准备代次）及服务端推导的环境属主。它不接受调用方路径、镜像、
命令或 URL。

只有原始 Manager 对当前未过期认领作业返回完全匹配的类型化确认，才能原子地把任务设为
`running`、环境设为 `ready`、作业设为 `succeeded`。失败、超时、取消或陈旧确认都会保留候选
及两个排他锚点，并将状态置为 `uncertain`。

### `POST /api/internal/execution-environments/{environment_id}/tasks/{task_id}/stop`

使用相同的两个请求字段调度或重放任务级停止。可接受 `prepared`、`preparing`、`starting`、
`running`、`stopping` 与 `uncertain` 任务。首次停止只推进一次授权代次，同时保留潜在候选；
恢复停止不会再次推进。停止意图提交后，该路由才会取消中继服务流；它绝不会释放任务。

Manager 收到 `stop_native_task` wire kind 与已存储任务身份。成功停止证明必须同时包含环境观察
为 `absent`、`exited` 或 `dead` 的确认，以及 `preparation_quiescent: true`。控制面会原子记录
正向证明并把环境标为 `replaced`，但任务仍保持 `stopping` 并继续独占工作区。失败、过期或未
静默的停止会令任务和环境变为 `uncertain`，且不产生释放证明。之后应用仍须以当前授权代次、
候选代次和精确停止作业围栏显式 release。

当本地检出根、Docker daemon 可见根以及现有执行环境配置均已配置时，原始 Manager 可执行
`prepared` 任务的类型化启动。容器只挂载持久状态卷和 UUID 推导的任务树到该绑定的检出目录；
在探测状态目录与任务树可写之前，必须校验精确任务标签、用户、工作目录和两个挂载，详见
[原生任务工作区目录](native-task-workspace-directory.zh-CN.md)。`running` 日志允许
验证后重放，不重建容器，也不将任务修改过的文件与种子比较。被中断的 `starting` 日志会保留且不会
自动重试。

原生停止会先持久化代次屏障，再检查 Docker，并等待共享环境锁和任务工作区锁，以证明准备写入已静默。
它只验证 Manager 所有的日志根目录，不读取或重写任务创建的后代文件；同时校验原生任务标签与挂载，
并对现存候选执行停止/删除后重新检查。只有完成这些检查并确认状态为 `absent`、`exited` 或 `dead`，
才会返回正向结果。Docker 状态不确定或身份不匹配时失败关闭，并保留屏障。停止会保留任务检出和状态卷；
任务释放仍是单独的显式操作。`execution_environments_ready` 仍为 false，默认服务路由不会切换到原生任务执行。

### Runner Manager 配置

准备功能**默认关闭**。原生启动还要求配置现有执行环境配置和 `RUNNER_NATIVE_TASK_HOST_ROOT`：

| 变量 | 含义 |
| --- | --- |
| `RUNNER_NATIVE_TASK_ROOT` | 本地现存、归 Manager 所有且权限为 `0700` 的绝对检出根目录。未设置则关闭原生准备、启动和停止。 |
| `RUNNER_NATIVE_TASK_HOST_ROOT` | Docker daemon 可见的规范化绝对路径；原生启动和验证现存停止候选时必需，绝不从其他根目录推断。 |

本地根目录保留原始路径并在使用时重新验证，因此符号链接别名会被拒绝。daemon 根不会被规范化，
也不要求在 Manager 本地存在。两个根都不会回退到工作区或规范项目根目录；检出目录和 daemon
挂载源只由可信环境与任务 UUID 推导：

```text
<本地根>/<environment_uuid>/<task_uuid>/tree
<daemon根>/<environment_uuid>/<task_uuid>/tree -> /workspace/bindings/<workspace_binding_id>
```

新目录权限为 `0700`、新文件为 `0600`，全部以 `create_new` 创建。暂存树发布前会重新计算哈希并与
已封存清单比对。`prepared` 日志会在任何 Docker 启动前原子推进到 `starting`；只有容器身份检查和
状态/任务目录可写探测均成功后才推进到 `running`。被中断的 `preparing` 或 `starting` 日志会保留，
并需要显式协调。`running` 重放会验证日志、容器身份和挂载，不会重写或重新计算任务修改过的文件。

## 本阶段的非目标

任务产物清理/导出、就绪激活和默认服务路由均不在本阶段范围内；停止不会自动释放任务。本阶段继续保持
`execution_environments_ready` 为 false，也不切换默认服务路由。Manager 启动和停止测试使用 fake Docker CLI；
不声称通过真实 Docker 或远程运行时验收。应用 API 不暴露状态卷、凭据、宿主路径、镜像、命令或 URL，
任何操作都不会写入遗留规范根目录。
