# 资源工作区绑定解析

[English](resource-workspace-binding.md)

应用在**不获取遗留写租约**的前提下，解析某个资源工作区的工作区绑定。

```
应用 ──HTTPS──▶ 控制面 ──▶ workspace_bindings 行
                  │
                  └─ 仅绑定身份；无租约、无原生任务、无执行
```

该端点由应用身份鉴权，且无请求体。它复用与资源租约端点相同的「查找或创建」解析器，
因此绑定身份是稳定的：同一应用、资源类型与资源标识总是解析到同一个绑定；首次解析
通过该解析器创建资源工作区目录。

## `POST /api/internal/resource-workspaces/{resource_kind}/{resource_id}/binding`

为调用方应用解析并在首次访问时创建某个资源工作区的活跃绑定。

| 路径参数 | 含义 |
| --- | --- |
| `resource_kind` | 资源工作区类型，例如 `artifact`。任何当前合法的类型都可接受。 |
| `resource_id` | 资源工作区标识。 |

响应即该绑定行：`workspace_binding_id`、应用 id、`workspace_key`、资源身份、
工作区路径，以及保持不变的 `write_lease_owner` / `write_lease_acquired_at` /
`write_lease_expires_at` 字段。

| 状态码 | 含义 |
| --- | --- |
| `200` | 已解析到活跃绑定。 |
| `400` | 某个路径参数不是合法的资源键。 |
| `401` | 缺少或错误的 Bearer 令牌，或该绑定不是活跃状态。 |
| `404` | 调用方应用不存在。 |
| `500` | 无法创建工作区。 |

## 主体隔离执行

经过应用认证的资源请求可通过 `X-DF-Runner-Isolation: principal` 选择隔离执行。分别提供一个 `X-DF-External-User-Id`、`X-DF-Workspace-Key` 和 `X-DF-Lease-Owner`；外部用户与租约所有者必须相同，工作区键必须指向资源。现有写租约和权限范围检查仍然有效。执行需要 Docker 后端的运行管理器。主体停止围栏对该身份永久有效；新的生命周期需要新身份。不带隔离请求头的资源工作区请求返回 `403`（`resource workspace requires an isolated principal`）。个人非资源应用工作区保持其归属行为。工作区重建须显式执行；认证流程不会重置或改写工作区内容。

## 工作区读取

直接文件读取无需写租约即可使用。在运行器容器内执行的读取（`glob`、`grep`）会在派发前锁定绑定行：锁定行必须仍匹配该绑定与应用、保持相同的工作区根目录，并且处于活跃且未归档状态。绑定缺失、已归档、重置中或受到其他保护（包括活跃的原生任务、作用域会话）时，搜索会以 `workspace is not available for reads` 拒绝。只读搜索从不获取写租约，锁会一直持有到运行器派发完成，包括首次 Running 确认。

## 运行器容器工作区挂载

运行器容器只在 `config.workdir` 挂载一个工作区目录，且必须是 manager 受信任 `WORKSPACE_ROOT` 的严格后代。根目录本身、父目录或兄弟目录、根之外路径、非目录、含符号链接组件的路径，以及包含逗号、NUL 或控制字符的值，都会在任何 Docker 命令执行前被拒绝。容器工作目录必须是规范化的绝对非根路径。

复用和启动已有的普通所有者容器时，检查到的挂载源必须与计算出的子目录完全一致。挂载缺失、等于根目录、位于根之外或其他不一致的容器会保持运行并以 incompatible-workspace-mount 错误拒绝；启动协调同样会跳过它。此类容器需要运维人员检查并手动删除，且删除时不得存在活动的 Shell 会话。私有执行环境容器保持现有的协调行为。

## 主体停止和协议发现

`GET /api/internal/principal-workspace-capabilities` 需要应用 Bearer 凭据，返回 `protocol_version: 1`；该版本描述服务器协议，不代表运行器可用。

`POST /api/internal/resource-workspaces/{resource_kind}/{resource_id}/workspace-sessions/stop` 需要应用 Bearer 凭据、`workspace.shell` 权限、`X-DF-Runner-Isolation: principal` 和匹配的外部用户、工作区键及租约所有者请求头。JSON 请求体为 `{ "expected_lease_owner": "..." }`，其值须与外部用户及租约所有者相同。成功响应确认该主体已停止，并指明是否释放了租约；其他所有者或空租约不会被修改。停止未确认时返回可重试错误，租约保留但永久主体围栏可能已生效。新的生命周期需要新身份。

## 工作区 Git 上游

管理员通过
`GET|POST /api/admin/workspace-bindings/{binding_id}/git-upstreams`、
`PATCH|DELETE …/{upstream_id}` 以及 `POST …/{upstream_id}/enable|disable`
按工作区绑定管理权威上游元数据。`remote_alias` 是严格的 ASCII 标识符（最多
64 个字符，以字母或数字开头）。`target_url` 是带主机、不含用户信息、查询串、
片段或控制字符的绝对 `http` 或 `https` Smart HTTP URL。已归档绑定仍可列出；
所有变更都要求绑定处于活跃状态，否则返回 `409`。未知或属于其他绑定的上游 id
返回 `404`，同一绑定内重复别名返回 `409`。

凭据只写：响应仅暴露 `has_credential`，绝不返回用户名或密钥。上游凭据按明确的
产品决定以明文存储在数据库中；创建或更新凭据无需任何运维密钥或额外配置。旧加密
格式写入的行仍可被识别，但绝不会被解密：在管理员重新提供凭据之前，其上游请求会
以 `credential_resupply_required` 失败关闭，重新提供的凭据会替换旧行。`enabled`
只是元数据：它不代表网络可达，也不保证 Git 推送本身能够成功。

## 原生 Git 网关

设置 `DF_GIT_PUBLIC_BASE_URL` 后，已保存的上游别名会成为普通工作区容器中的原生
Git 远端。
`GET /git/{binding_id}/{remote_alias}/info/refs` 只接受一个
`service=git-upload-pack|git-receive-pack` 查询；
`POST /git/{binding_id}/{remote_alias}/git-upload-pack|git-receive-pack` 负责代理
pack 流。目标只来自管理员保存的 URL，调用方不能选择主机、路径前缀、额外查询或
上游 URL。

能力签名密钥只生成一次并持久化在数据库中，因此并发实例共享同一密钥，重启后仍
保留，无需运维配置。Runner 使用 Basic 用户名 `df-git` 和短期签名能力令牌，令牌
限定到工作区绑定及别名代次。上游凭据只在网关请求内读取，绝不进入 runner、argv、
`.git/config` 或工作区。客户端 Cookie、Authorization、代理和转发头都会被清除；重定向、上游错误
体、`Location`、`Set-Cookie` 与 `WWW-Authenticate` 也不会转发。请求与响应 pack
均流式处理，默认上限 256 MiB（`DF_GIT_GATEWAY_MAX_BODY_BYTES`，最大 1 GiB），并
带有连接、响应头和总流时限。

每次 shell 或 `write_stdin` 派发都会重新签发能力令牌，并写入 `noexec` 的小型
tmpfs 凭据文件。Manager 只配置已启用的 DF 别名，绝不会删除用户无关的远端。禁用、
删除、别名/目标变更、凭据替换或代次变化都会使旧令牌在下一次请求时失效。基础 URL
缺失时网关固定返回 `503`，原生 Git 不可用，但普通 Shell 工具仍可工作。签名密钥初始化
失败会终止启动，而不会退回到未签名的网关。
`enabled` 仍然只是元数据；请求成功仍取决于 runner 是否可达已保存的目标。

## 应用内部的上游管理

应用 Bearer 凭据无需管理员浏览器会话即可管理自身绑定的上游元数据：

- `GET /api/internal/workspace-bindings/{binding_id}/git-upstreams`
- `POST /api/internal/workspace-bindings/{binding_id}/git-upstreams`
- `PATCH /api/internal/workspace-bindings/{binding_id}/git-upstreams/{upstream_id}`
- `DELETE /api/internal/workspace-bindings/{binding_id}/git-upstreams/{upstream_id}?lease_owner=...`

读取使用应用主体并解析精确应用与外部用户主体的绑定；绑定不存在或属于其他应用时返回 `404`。
资源绑定使用隔离主体请求头（`X-DF-Runner-Isolation: principal`、相互匹配的
`X-DF-External-User-Id` 与 `X-DF-Lease-Owner`，以及指向该资源的 `X-DF-Workspace-Key`）鉴权。
调用方身份是真实外部用户；存储的 `__resource__` 标记仅限内部，绝不要求客户端发送或返回给客户端。
解析出的作用域必须精确命名该绑定，且调用方租约必须是当前有效租约；应用、绑定或主体不一致，
以及过期、陈旧或由他人持有的租约，在读取和写入时都会被拒绝。能力端点使用相同的请求头与权限规则。
外部用户请求头对所有调用方均为必需；隔离、工作区键和租约所有者请求头在生成的契约中为可选，
因为个人调用方不发送它们。个人绑定仍使用存储主体校验，且无需写租约。
写入还要求现有工作区写权限，以及 JSON 请求体（删除时为查询参数）`lease_owner` 指定的当前写租约；
资源绑定的该值还必须等于已认证租约。
绑定行会在校验该权限之前加锁，并在同一事务内完成修改，因此与请求竞争的租约交接无法通过快照校验。
个人绑定沿用现有个人写策略；资源绑定必须持有未过期的当前租约。

创建与更新复用管理员 API 相同的别名、URL 与只写凭据校验类型。凭据以明文存储且绝不返回；
响应只包含 `has_credential`、别名、目标 URL、`enabled` 与 `capability_generation`。
每次成功修改都会递增代次，旧能力令牌随即失效。重复别名、非活动绑定与过期租约返回 `409`；
其他应用或主体返回 `403`；跨绑定上游 id 返回 `404`。

## 归属边界

解析绑定**不授予任何权限**：

- 不获取、不续期、不清除也不接管写租约——租约端点仍是唯一的遗留租约权限来源，
  已存在的属主会被原样返回；
- 不登记、不调度、不释放原生任务，解析之后活跃的原生任务仍然独占其工作区；
- 不推断任何容器、Manager 或代次状态。

原生任务登记仍是唯一权威：绑定上只要存在任何遗留写属主（含已过期的）即被拒绝，
并且每个工作区同时只有一个活跃任务。已解析的绑定仍可能在登记时被拒绝。

响应包含内部工作区路径，属于应用与应用之间的接口，不是面向用户的数据。