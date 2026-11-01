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

经过应用认证的资源请求可通过 `X-DF-Runner-Isolation: principal` 选择隔离执行。分别提供一个 `X-DF-External-User-Id`、`X-DF-Workspace-Key` 和 `X-DF-Lease-Owner`；外部用户与租约所有者必须相同，工作区键必须指向资源。现有写租约和权限范围检查仍然有效。执行需要 Docker 后端的运行管理器。主体停止围栏对该身份永久有效；新的生命周期需要新身份。不提供隔离请求头时，运行器归属保持不变。

## 主体停止和协议发现

`GET /api/internal/principal-workspace-capabilities` 需要应用 Bearer 凭据，返回 `protocol_version: 1`；该版本描述服务器协议，不代表运行器可用。

`POST /api/internal/resource-workspaces/{resource_kind}/{resource_id}/workspace-sessions/stop` 需要应用 Bearer 凭据、`workspace.shell` 权限、`X-DF-Runner-Isolation: principal` 和匹配的外部用户、工作区键及租约所有者请求头。JSON 请求体为 `{ "expected_lease_owner": "..." }`，其值须与外部用户及租约所有者相同。成功响应确认该主体已停止，并指明是否释放了租约；其他所有者或空租约不会被修改。停止未确认时返回可重试错误，租约保留但永久主体围栏可能已生效。新的生命周期需要新身份。

## Git 同步

`POST /api/internal/resource-workspaces/{resource_kind}/{resource_id}/git/sync` 需要资源写租约、shell 权限和网络访问。提供 `remote_url`，可选提供 `branch`。可选的 `preserve_existing` 默认为 `false`：已有检出必须干净，并仅执行快进拉取。为 `true` 时，已有检出不会获取或拉取，工作内容和分支保持不变。空工作区仍正常克隆。成功同步会将受管理的 origin 拉取和推送地址设为提供的地址，并返回当前 HEAD、分支和修改状态。

## 归属边界

解析绑定**不授予任何权限**：

- 不获取、不续期、不清除也不接管写租约——租约端点仍是唯一的遗留租约权限来源，
  已存在的属主会被原样返回；
- 不登记、不调度、不释放原生任务，解析之后活跃的原生任务仍然独占其工作区；
- 不推断任何容器、Manager 或代次状态。

原生任务登记仍是唯一权威：绑定上只要存在任何遗留写属主（含已过期的）即被拒绝，
并且每个工作区同时只有一个活跃任务。已解析的绑定仍可能在登记时被拒绝。

响应包含内部工作区路径，属于应用与应用之间的接口，不是面向用户的数据。