# 冷启动原生任务

原生任务可以从真正冷的环境启动：一次从未指定 Manager 的公开 `ensure`、没有预热置备，
也不需要对环境行做任何手工写入。

## 冷形态

公开 `ensure` 会创建一个 `assigned_manager_id = NULL` 且没有候选代次的 absent 环境。
本路径针对的就是这一行；在原生启动之前，产品中没有任何其他环节会向它写入 Manager。

| 步骤 | 对 Manager 决定了什么 |
| --- | --- |
| `ensure` | 什么都不决定，环境保持未绑定。 |
| `admit` | 把一个可用 Manager 固定到**任务**行上，环境不受影响。 |
| `seed` | 什么都不决定。 |
| `prepare` | 按任务的固定结果调度，并接受该 Manager 的回执；环境保持未绑定。 |
| `start` | 原子地把环境绑定到任务自身的 Manager。 |

## 两个判定允许什么

两处关卡共用同一条规则：环境没有 Manager **且**仍为 `absent`、没有候选代次时才允许冷绑定；
已绑定的环境只有在它指向的就是这个 Manager 时才被接受。其余一律冲突，绝不会替换成其他 Manager。

- 原生启动（`src/sql/execution_environments/tasks/start/mark_environment_starting.sql`）写入
  `assigned_manager_id = COALESCE(assigned_manager_id, $6)`，并以
  `(assigned_manager_id = $6 OR (assigned_manager_id IS NULL AND state = 'absent' AND container_generation IS NULL))` 守卫。
- 准备完成回执（`src/db/execution_environments/tasks/prepare/complete/transaction.rs`）
  对 Manager 的回执应用同一条规则，并在两个分支中都保留“无候选代次”的要求。

外来 Manager 绝不会被改指向：非 NULL 的不匹配在两处关卡都始终是冲突。

## 原子性与回滚

启动在**同一条语句、同一事务**内绑定 Manager，因此作业插入、任务状态推进与环境赋值要么
全部提交，要么全部不发生。其余所有证明都先检查——绑定资格、准备证明、环境候选、旧写入阻挡
与 Manager 可用性——在此之前不会持久化任何内容。若环境更新被拒绝或失败，事务回滚，
不会留下作业、任务状态或绑定。

## 未改变的部分

作业类型、幂等令牌、期望代次、任务 UUID 以及 Manager 的目录配置与热路径完全一致。
令牌重放路径、现有的原始 Manager 完成校验与服务代次围栏均未改动。
admission、lifecycle、stop、release、seed 策略均无变化，也没有 schema、迁移、
API 字段或依赖变更：导出的 OpenAPI 文档与上一版本逐字节相同。

## 相关文档

- [持久化原生任务意图](execution-environment-tasks.zh-CN.md)
- [资源工作区绑定](resource-workspace-binding.zh-CN.md)
