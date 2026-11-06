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
| `resource_kind` | 资源工作区类型，例如 `code_project`。任何当前合法的类型都可接受。 |
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

## 主体停止和协议发现

`GET /api/internal/principal-workspace-capabilities` 需要应用 Bearer 凭据，返回 `protocol_version: 1`；该版本描述服务器协议，不代表运行器可用。

`POST /api/internal/resource-workspaces/{resource_kind}/{resource_id}/workspace-sessions/stop` 需要应用 Bearer 凭据、`workspace.shell` 权限、`X-DF-Runner-Isolation: principal` 和匹配的外部用户、工作区键及租约所有者请求头。JSON 请求体为 `{ "expected_lease_owner": "..." }`，其值须与外部用户及租约所有者相同。成功响应确认该主体已停止，并指明是否释放了租约；其他所有者或空租约不会被修改。停止未确认时返回可重试错误，租约保留但永久主体围栏可能已生效。新的生命周期需要新身份。

## Git 同步

`POST /api/internal/resource-workspaces/{resource_kind}/{resource_id}/git/sync` 需要资源写租约、shell 权限和网络访问。请求提供旧的受作用域 broker `remote_url`、与之匹配的 `code_project_id`、非 nil 的 `authorization_generation`，以及可选的 `branch`。服务端在私有路径中拆分受作用域 URL，scoped 密码不会进入模型命令、任务负载、数据库行、结果、错误或仓库配置。

已有检出且 `preserve_existing: true` 时，仅用固定的无凭据沙箱探针检查：不获取、不拉取、不访问网络、不写配置，也不创建新任务。包含 include、不安全配置形态、外部对象目录或带凭据的旧 origin URL 的仓库返回 `409` 和 `legacy_git_cleanup_required`，文件保持不变；该响应保留工作内容，但不代表旧工作区迁移已完成。

默认的 `preserve_existing: false` 对已有检出执行干净快进。请求必须显式给出规范分支，探针必须报告工作区干净且处于该分支，实际 HEAD 会成为任务的期望基线。脏工作区、detached、分支不匹配或其他不安全形态返回 `409` 且保持不变。受信任 manager 持有 binding 写栅栏并拒绝任何存活的绑定 Shell 会话；它只导入已校验的 bundle，通过 compare-and-set 引用更新和 `read-tree` 发布精确后代，并保留其他引用与历史。非快进或基线不匹配会拒绝，引用与工作区均不改变。非空且非仓库目录仍返回 `409`。

持久化的首次克隆不携带 expected base，行为保持不变。任务为 `pending` 或 `claimed` 时，路由只在原始等待预算内轮询同一任务，绝不新建任务、重置凭据期限或重放工作。任务为 `uncertain` 时，协调只读且仅凭证明：manager 读取已完成、已发布且清理确认的 receipt，并独立检查干净的实际 HEAD。只有证明完全匹配时，服务端才用严格的结果 JSON 把该 uncertain 行 compare-and-set 为 succeeded；缺失、损坏、不匹配、过期、忙或脏的证明都返回明确的 `503`，绝不触发外部 Git 写、fetch、pull、push、重放、凭据注册或新任务。

目标为空且没有 `.git` 时，为已经观测到服务该 binding 的 manager 准入一个持久首次克隆任务；`branch` 必填且必须符合共享规范。没有实时观测时，路由执行一次无凭据 bootstrap 探针并重新读取一次观测。零个或多个实时 manager 返回 `503`。首次克隆成功返回 `cloned: true`、`dirty: false`、规范分支和已验证 head。pending、uncertain 或超时工作不会被重放。

## 归属边界

解析绑定**不授予任何权限**：

- 不获取、不续期、不清除也不接管写租约——租约端点仍是唯一的遗留租约权限来源，
  已存在的属主会被原样返回；
- 不登记、不调度、不释放原生任务，解析之后活跃的原生任务仍然独占其工作区；
- 不推断任何容器、Manager 或代次状态。

原生任务登记仍是唯一权威：绑定上只要存在任何遗留写属主（含已过期的）即被拒绝，
并且每个工作区同时只有一个活跃任务。已解析的绑定仍可能在登记时被拒绝。

响应包含内部工作区路径，属于应用与应用之间的接口，不是面向用户的数据。