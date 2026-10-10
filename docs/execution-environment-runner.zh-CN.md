# 执行环境 Runner 原语（issue 739/740/742 P1）

用于创建隔离持久态环境、确认容器完全停止，并在专用状态根内提供持久
generation 屏障与跨进程预留的已测试 Docker 原语。
仅做加法：不含 OpenCode 集成、公开代理、路由、迁移或线上启用。

## Owner 与 DTO

- `RunnerOwner::ExecutionEnvironment { environment_id, generation }`
 （`crates/runner-protocol`）。`environment_id` 为 UUID 字符串；
  `generation` 为 `>= 0`。
- `stable_key()` 为 `execution_environment:<uuid>:<generation>`；
  `container_name()` 由其派生，因此各 generation 容器名不同。
  加锁使用仅含环境 id 的 `lock_key()`——按 generation 加锁无法
  串行化替换。
- `EnsureExecutionEnvironmentRequest { owner }` 与
  `StopExecutionEnvironmentRequest { owner }` 仅携带身份
 （`deny_unknown_fields`）。镜像、宿主机路径、卷、环境变量、
  密钥与端点一律不接受。
- `EnsureExecutionEnvironmentAck { owner, container_name, state }`
  回显请求 owner 与观察到的 `running`。
  `StopExecutionEnvironmentAck { owner, container_name, confirmed, observed_state }`
  回显 owner 与 `absent`/`exited`/`dead`/`removed` 或观察到的状态。
  不含密钥、宿主机路径或卷细节。

## 受信配置（默认禁用）

- `ExecutionEnvironmentProfile`（`runtime/execution_environment`）：
  `enabled=false`、`state_mount_target=/state`、`stop_timeout=20s`、
  `operation_deadline=60s`、`volume_uid/gid=None`（未显式配置则阻塞）、
  `state_dir=None`（未显式配置则阻塞）。
- 仅通过进程环境变量启用（`RUNNER_EXECUTION_ENVIRONMENTS_ENABLED`、
  `..._STATE_MOUNT_TARGET`、`..._STOP_TIMEOUT_SECS`、
  `..._OPERATION_DEADLINE_SECS`、`..._VOLUME_UID/GID`、`..._STATE_DIR`）。
  操作总时限接受 1–3600s，超出范围回退到 60s 默认值；
  非正的配置值在任何工作前即被拒绝。
  镜像、`pids/memory/cpu`、`runtime_class` 与网络均来自现有 manager
  配置；请求不可覆盖。
- `state_dir` 仅从 `RUNNER_EXECUTION_ENVIRONMENT_STATE_DIR` 读取，
  默认未设置即禁用，必须是专用、持久、由 manager 控制且不挂载进
  agent 工作区的根目录。未配置时环境操作在任何 Docker 调用前失败
  关闭，绝不回退到临时状态。
- 卷名为 `df-env-<uuid-simple>`（仅由 UUID 派生，跨 generation 稳定）。
  创建要求显式 uid/gid；禁止猜测。

## 总时限与属主校验

- 单个 ensure/stop 由一条单调操作总时限覆盖环境锁与所有 Docker 命令；
  每条命令只获得剩余预算。预算耗尽即报错，绝不退化为无界等待；
  超时按失败关闭处理，绝不视为缺席。有界命令复用共享命令执行器，
  因此到时限会先 kill 并回收已派生的 Docker CLI。
- 每个成功确认与每个破坏性命令都要求容器的 owner、environment-id 与
  generation 标签与请求一致。缺失、畸形或异主标签一律拒绝，
  不执行任何 stop/rm。`stop_timeout` 仍用于限制优雅 `docker stop`。

## 持久状态根与 generation 屏障

- 专用根目录（`state_dir`）按环境存放 `barrier.json` generation 屏障
  与 `lock` 预留，路径仅由环境 UUID 派生，请求无法选择或逃逸路径。
- 屏障持久化非负单调 generation 高水位与可选的已停止 generation。
  写入为原子操作：同目录临时文件、文件 sync、rename、目录 sync。
  缺失状态视为空屏障；存在但损坏、非法、版本不支持或超限的文件
  按失败关闭处理，绝不重置。
- 预留是一个带显式持有者 token 的 `create_new` 文件。已存在或崩溃
  残留的预留一律报错而非被抢占：不做陈旧 PID 清理，也不基于 `Drop`
  删除；释放仅在文件仍携带本持有者 token 时删除该文件。
  普通错误退出释放自己的预留；崩溃或取消会保留预留，等待运维显式
  恢复。共享同一根目录的两个协作 manager 因此无法同时 ensure/stop
  同一环境，崩溃残留文件会阻塞两条路径。
- 预留覆盖屏障读改写与整个 Docker 操作。原有进程内环境互斥锁保留为
  第一层更廉价的串行化。

## 加固容器

`build_environment_run_argv` 仅生成：

- `--name <generation容器>`、`--user <uid:gid>`、
  `--label desk-foreman.owner/managed/manager/execution-environment/generation`、
  `-e DF_EXECUTION_ENVIRONMENT/DF_GENERATION`、
  `-v <volume>:/state`、`-w /state`、
- `--read-only`、`--tmpfs /tmp:rw,nosuid,size=256m`、`--cap-drop ALL`、
  `no-new-privileges`、`--pids-limit/--memory/--cpus`、
  已配置时加 `--runtime`，受信配置未启用网络时加 `--network none`，
  最后为 `<image> sleep infinity`。

无 `-p`/`--publish`，无宿主机工作区绑定，无全局根挂载。
任务工作区保持不挂载；`docker volume create` 仅由运行时代码/测试
经 fake harness 调用，开发期间不以 live CLI 执行。

## Ensure

- 校验 owner，要求启用配置、显式属主与已配置的 `state_dir`；派生卷名，
  先加进程内环境锁，再取得跨进程预留，并在任何 Docker 调用前依据
  持久屏障拒绝已关闭或更低的 generation。随后将本次尝试持久化为新的
  高水位，创建卷，再执行
  `docker ps --filter label=desk-foreman.execution-environment=<id>`
  ——全部在操作总时限内完成。
- 已被持久停止的 generation（`stopped_generation >= 请求`）或低于
  已尝试高水位的 generation 在任何 Docker 调用前即拒绝，
  因此迟到的 ensure 无法重启 stop 已关闭的 generation。
- 同 generation `running` → 仅在容器标签与请求 owner 一致时幂等成功
  （不再 `run`）。同 generation 已停止 → `start` 后确认，且同样校验
  owner（仅当无持久屏障关闭它时）。
- 其他 generation 为 `running`/`restarting`/`paused`/`unknown`
 （除 `exited`/`dead`/`removed` 外一律）拒绝创建。
  绝不为强制通过而删除容器。
- 以加固参数创建，随后 inspect 并要求 owner 匹配才确认 `running`
  （如触发纠正性 `start`，之后再次校验）。

## Stop

- 校验 owner，要求已配置 `state_dir`，并按 `environment_id` 加锁；
  与 ensure 不同，stop 不要求启用开关，因此即使创建被禁用，
  已有环境仍可停止。
- 先 inspect。明确缺席的容器直接确认，不执行任何破坏性命令；
  存在的容器仅在 owner/environment/generation 标签匹配后才可触碰。
  异主或畸形标签在任何 `stop`/`rm` 之前失败，且不记录屏障。
- 标签匹配（或容器明确缺席）后，在任何破坏性命令或确认之前先把
  已停止 generation 持久化到屏障，因此失败或超时的 stop 仍会向迟到的
  ensure 关闭旧 generation，但绝不返回已确认的 ack。
- 属主匹配且已停止的容器直接确认，不执行破坏操作。属主匹配且仍在
  运行的容器以有界 `stop`（受 `min(stop_timeout, 剩余预算)` 限制）
  加 `rm`/`rm -f` 升级终止，随后再次 inspect。
  不检查 `active_ops`——写者必须被终止，而非被跳过。
- 仅当最终 inspect 证明 `exited`/`dead`/`removed` 或以
  `No such container/object` 明确判定缺席时才确认。
  传输、解析、超时或权限失败永不视为缺席；仍在运行则返回
  `confirmed=false`。绝不 `volume rm`。
- Ack 的 owner/generation 与请求一致。持久停止意图与实际停止确认
  保持区分：屏障并不声称容器已被杀死。

## 兼容与限制

- 旧 `workspace_runners` 与 admin 会话明确拒绝/跳过环境 owner；
  不在此持久化或上报。`execution_environments_ready` 保持 `false`；
  `runtime_kind` 保持不透明（DF 永不按 OpenCode/ACP 分支）。
- Docker 标签扫描只是防护，不是原子分布式锁。
  多 manager 或多 engine 仍需后续生命周期阶段的 durable affinity/CAS；
  在此之前仅限单控制器使用，并保持能力禁用。
- 持久预留只覆盖共享同一状态根的同机协作进程，不覆盖不同 Docker
  主机。崩溃残留的预留按失败关闭处理，需运维显式恢复，
  绝不依据 PID 启发式自动删除。状态根必须专用、持久、由 manager
  控制，且位于 agent 工作区挂载之外。
- 开发环境无 Docker daemon 访问；真实卷权限、子进程终止与跨进程
  运行时测试留待后续启用。本阶段不启用 live 路由或原生写。
