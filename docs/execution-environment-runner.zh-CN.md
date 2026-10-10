# 执行环境 Runner 原语（issue 739/740/742/749 P1）

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

- `ExecutionEnvironmentProfile`（`runtime/execution_environment/profile`）：
  `enabled=false`、`state_mount_target=/state`、`stop_timeout=20s`、
  `operation_deadline=60s`、`volume_uid/gid=None`（未显式配置则阻塞）、
  `command`/`state_workdir` 未设置（未显式配置则阻塞）、
  `state_dir=None`（未显式配置则阻塞）。
- 仅通过进程环境变量启用（`RUNNER_EXECUTION_ENVIRONMENTS_ENABLED`、
  `..._STATE_MOUNT_TARGET`、`..._STOP_TIMEOUT_SECS`、
  `..._OPERATION_DEADLINE_SECS`、`..._VOLUME_UID/GID`、`..._STATE_DIR`、
  `..._COMMAND`、`..._STATE_WORKDIR`）。操作总时限接受 1–3600s，
  超出范围回退到 60s 默认值；非正的配置值在任何工作前即被拒绝。
  镜像、`pids/memory/cpu`、`runtime_class` 与网络均来自现有 manager
  配置；请求不可覆盖。
- 配置始终可选且永不致命。缺失的可选项使 profile 对 ensure 不可用；
  显式畸形的值保留为降级 profile，绝不静默回退到默认值。
  每条拒绝信息只说明配置项与被违反的规则，绝不回显配置值。
- `..._COMMAND` 是 JSON argv 数组，用 `serde_json` 解析，不是 shell
  字符串：没有 eval，每个 token 保持原始字节，包括内嵌空格与 shell
  元字符。要求为非空数组，参数个数不超过 64，总字节不超过 16KiB，
  不含 NUL，且第 0 个元素为非空可执行文件。可执行文件之后的普通空参数
  仍然合法。
- `..._STATE_WORKDIR` 为 ensure 必填，并相对 `..._STATE_MOUNT_TARGET`
  做纯词法校验：必须是绝对路径，不含控制字符，不含 `.`/`..` 分量，
  不含重复斜杠与结尾斜杠，且必须是按分量边界严格下降的子路径
  （因此 `/stateful` 不算在 `/state` 之内）。这里不做宿主机文件系统
  规范化，该根描述的是容器挂载点，而非 manager 本地路径。
- `state_dir` 仅从 `RUNNER_EXECUTION_ENVIRONMENT_STATE_DIR` 读取，
  默认未设置即禁用，必须是专用、持久、由 manager 控制且不挂载进
  agent 工作区的根目录。未配置时环境操作在任何 Docker 调用前失败
  关闭，绝不回退到临时状态。
- 卷名为 `df-env-<uuid-simple>`（仅由 UUID 派生，跨 generation 稳定）。
  创建要求显式 uid/gid 且 `uid > 0`；禁止猜测，agent 与探针都不以
  root 运行。

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
  `-v <volume>:/state`、`--workdir <状态工作目录>`、
- `--read-only`、`--tmpfs /tmp:rw,nosuid,size=256m`、`--cap-drop ALL`、
  `no-new-privileges`、`--pids-limit/--memory/--cpus`、
  已配置时加 `--runtime`，受信配置未启用网络时加 `--network none`，
  最后为 `<image>` 后接配置的 command argv，逐 token 原样传递。

配置的命令即容器 `CMD`，镜像自身的 `ENTRYPOINT` 仍决定实际执行内容，
因此所配置的镜像必须提供能接受该形态命令的 entrypoint。这里不臆造
entrypoint 覆盖，请求参数也不控制可执行文件、镜像或任何宿主机路径。

无 `-p`/`--publish`，无宿主机工作区绑定，无全局根挂载。
任务工作区保持不挂载；`docker volume create` 仅由运行时代码/测试
经 fake harness 调用，开发期间不以 live CLI 执行。

## 可选私有服务配置（issue 749 P2a）

被服务的环境还会加入一个受信 Docker 网络，并从环境变量读取一份按
generation 生成的凭据。本子阶段只提供启动前提：不会拨号、不会打开套接
字，也不存在中继。

- 四项配置必须全有或全无：
  `RUNNER_EXECUTION_ENVIRONMENT_SERVICE_NETWORK`、
  `..._SERVICE_PORT`、`..._SERVICE_AUTH_USERNAME`、
  `..._SERVICE_AUTH_ENV_NAME`。全不设置时环境行为与引入服务支持之前完全
  一致；只设置其中一部分属于降级配置——manager 启动永不致命，但 ensure
  会在任何 Docker 工作之前拒绝，而不是悄悄启动一个无法服务的环境。
- 网络名必须是有界 ASCII Docker 名称，端口为 `1..=65535`，basic auth
  用户名是有界可打印 ASCII 且不含冒号，变量名为 POSIX 标识符。
  诊断信息只说明配置项，绝不回显其值。
- `docker run` 只额外增加 `--network <name>` 与
  `--env-file <宿主机路径>`。绝不为 agent 发布端口，绝不挂载凭据文件，
  凭据值也绝不进入 argv。manager 部署必须已加入该网络；此处不创建或
  挂接网络。
- 凭据是工作区 `uuid` 生成的 UUIDv4，按 `(环境, generation)` 存放于
  持久屏障旁的
  `<state_dir>/<environment>/service/<generation>.env`，
  位于仅属主的目录中、模式 `0600`，通过 `create_new` 临时文件写入，
  fsync 后原子 rename。它在同一 generation 内稳定，因此重启会复用；
  不同 generation 与不同环境各不相同，因此不存在跨用户共享。
- 属主判定使用进程的**有效** uid，绝不使用真实 uid，也绝不以目录属主
  代替。若 procfs 无法提供，则该校验失败关闭——无法完成的属主校验绝不
  被当作通过。
- 目录链在每次创建与每次读取时都会重新校验：每个分量都必须是真实目录、
  非符号链接、由 manager 拥有，且不可被同组或其他用户访问。过于开放的
  目录会被拒绝并保持原样，而不是被悄悄收紧。
- 凭据文件每次读取都基于**已打开的句柄**重新校验，因此打开后被替换的
  路径无法改变被接受或被读取的内容：必须是普通文件、恰好 `0600`、
  由有效 uid 拥有、大小有界、经有上限的读取器读取，且恰好包含所配置的
  `NAME=token` 一行；符号链接在打开前即被拒绝。任何不满足的情况都会被
  拒绝且原样保留：绝不改权限、改写或删除。复用与重启要求既有凭据存在，
  否则失败关闭，而不是给已在运行的容器换上新值。只有创建分支会生成凭据，
  且发生在既有准入锁内、屏障之后、容器 run 之前。
- **显式前提：** 状态根是被信任的、由 manager 拥有且其他本地账户无法写入
  的目录树。标准库不提供 `O_NOFOLLOW` 打开，也不提供基于解析后句柄的比较，
  因此要完全消除「先检查后打开」的竞争窗口需要引入公开的 `libc`/`rustix`
  依赖，而它不是当前可用的工作区依赖。在上述前提下，先检查加基于句柄的
  校验成立；此处并不声称已消除该检查-打开窗口本身。
- agent 镜像需自行读取所配置的变量并强制 basic auth。manager 不在任何
  DF 侧、DTO、标签、卷或诊断中保存 agent 凭据；本阶段不构造 Basic 头，
  它随后续子阶段的中继客户端一并加入。

## 同容器状态探针

`running` 确认的含义不止于容器状态：受信镜像必须确实允许配置的非 root
用户在状态挂载内写入。

- `docker exec --user <uid:gid> --workdir <状态工作目录> <精确属主容器>
  sh -c <固定脚本>`。无辅助容器、无 root 权限、无新挂载、无新端口、
  无第二个写者。
- 脚本是固定的 manager 常量：在工作目录内创建私有 `mktemp` 文件、
  写入、删除，并在任何退出路径上再次删除。这是真实的
  创建/写入/unlink 观测，不是递归遍历或属主变更。它假定受信镜像提供
  POSIX `sh` 与 `mktemp`；不会注入任何宿主机二进制。
- 容器名、用户与工作目录以独立 argv 项传递，绝不插值进脚本，
  因此任何配置字符串都到不了 shell。探针输出永不进入 manager 诊断。
- 探针是普通有界 CLI 调用，不宣称在 CLI 超时时具备取消安全性，
  也不是停止证明。终止仍只由既有 stop 屏障负责。
- 探针成功只是可写性观测，不是安全边界，也不保证镜像 copy-up 保留
  属主。

## Ensure

- 校验 owner，再校验受信进程输入——启用配置、显式非 root 属主、
  合法命令与合法状态工作目录——全部先于任何磁盘或 Docker 工作。
  缺失或畸形的输入拒绝所有 ensure 分支（含复用），而 stop 仍可用。
  随后派生卷名，先加进程内环境锁，再取得跨进程预留，并在任何 Docker
  调用前依据持久屏障拒绝已关闭或更低的 generation。再将本次尝试
  持久化为新的高水位，创建卷，然后执行
  `docker ps --filter label=desk-foreman.execution-environment=<id>`
  ——全部在操作总时限内完成。
- 已被持久停止的 generation（`stopped_generation >= 请求`）或低于
  已尝试高水位的 generation 在任何 Docker 调用前即拒绝，
  因此迟到的 ensure 无法重启 stop 已关闭的 generation。
- 同 generation `running` → 先要求容器标签与请求 owner 一致，
  再要求状态探针确认可写，最后才幂等成功（不再 `run`）。
  同 generation 已停止 → `start`、重新校验 owner、运行探针，之后才确认。
- 其他 generation 为 `running`/`restarting`/`paused`/`unknown`
 （除 `exited`/`dead`/`removed` 外一律）拒绝创建。
  绝不为强制通过而删除容器。
- 以加固参数创建，随后 inspect 并要求 owner 匹配且状态为 `running`
  （如触发纠正性 `start`，之后再次校验），再运行探针，之后才确认。
- 探针失败是不确定性，不是停止证明：不删除、不停止、不自动重试，
  持久高水位保持已写入的内容，且在该候选可能仍在运行时不放行更高
  generation。控制器保留该不确定候选，直到执行显式的属主校验 stop。

## Stop

- 校验 owner，要求已配置 `state_dir`，并按 `environment_id` 加锁；
  与 ensure 不同，stop 不要求启用开关、命令、工作目录或属主，
  因此即使创建被禁用或配置有误，已有环境仍可停止。
  stop 从不运行状态探针。
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
- 开发环境无 Docker daemon 访问；生命周期确认并不等于运行时就绪：
  探针只是 fake CLI 与文件系统层面的观测，真实卷权限、copy-up 属主、
  所配置镜像的 `sh`/`mktemp`/entrypoint 前提，以及跨进程运行时测试
  都要留待后续启用。本阶段不启用 live 路由或原生写。
