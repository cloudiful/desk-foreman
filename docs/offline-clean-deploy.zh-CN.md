# 离线干净部署

[English](offline-clean-deploy.md) | 简体中文

本手册描述一种单实例维护部署：停止准入与执行主体，丢弃经明确批准的受管工作区
与运行时状态，然后以干净状态启动。它仅为文档：不授权执行，也不提供可直接复制
的破坏性命令。

## 警告与批准门槛

清理不可逆。在操作者具备以下条件前不得开始：

- 一份书面批准，明确目标实例以及要丢弃的确切状态；
- 当前数据库备份及恢复决策；
- 针对所选状态的实时清单，而不仅是本文档。

本文档不是经过生产验证的清单。所有部署相关取值都必须来自实际部署。

## 部署方式

- 部署采用单实例离线切换；不提供在线重置、滚动兼容、旧状态接管或恢复路径。
- 丢弃范围是所选受管工作区内容及运行时历史；相关许可记录保留在部署记录中。
- 登录会话不属于工作区历史，始终受保护。
- 未知或无法分类的状态一律保留，不得删除。

## 状态分类

### A. 已批准的可丢弃范围

受管工作区目录与所选运行时行只有在确切的路径、行、卷和表都被选择并核验后才
可清理。它不是盲目清除范围。

### B. 需要按部署逐项选择的候选表

每张表都需要明确选择、行数、保留处置和依赖核验。

Desk Foreman 工作区/运行时候选：

- `workspace_bindings`
- `workspace_runners`
- `runner_principal_admission`
- `runner_owner_stop_fence`
- `execution_environments`
- `execution_environment_jobs`
- `execution_environment_tasks`
- `execution_environment_task_seeds`
- `execution_environment_task_seed_files`
- `execution_environment_task_exports`
- `execution_environment_task_export_selection_files`
- `execution_environment_task_export_files`
- `execution_environment_task_export_missing_paths`

在处置运行时历史前，先静默并分类 pending 或运行中的工作。排队并不证明不会再
产生外部影响。

### C. HOLD：外部写入、审批或审计证据

这些记录应保留、归档或对账，不得纳入批量删除。

- `audit_log`
- 保留的惰性历史证据（表与行仍保留）

与所选状态相关的审批或对账证据同样保留。

### D. 受保护：本手册永不删除

- `users`
- `web_sessions`
- `mcp_tokens`
- `applications`
- `application_tokens`
- `runner_managers`

不得为了满足外键而清空或重建整个数据库。

## 必需的实时清单

删除前记录并复核以下内容：

1. 目标实例、运行版本、数据库身份、备份引用和指定维护负责人。
2. 所选表及行数、保留处置，以及每项选择的外键图检查结果。
3. 确切解析后的规范化主机与容器路径、卷名与挂载拓扑，以及当前任务所有权与
   共享检查。
4. 部署实际使用的 `WORKSPACE_ROOT`、`RUNNER_HOST_WORKSPACE_ROOT`、
   `RUNNER_EXECUTION_ENVIRONMENT_STATE_DIR`、`RUNNER_NATIVE_TASK_ROOT`、
   `RUNNER_NATIVE_TASK_HOST_ROOT`，以及每个环境 UUID 对应的卷。
5. 选中的工作区路径。某个根目录可能同时是私有凭据或无关项目的父目录：绝不
   凭推断删除挂载父目录，只删除所选工作区目录。

凭据归档不得进入工作区、模型流量或 Shell 输出。通过受支持的服务生命周期轮换
或保留机器凭据；绝不把秘密复制到命令行参数或临时文件。

## 停机

1. 先禁用外部入口与准入。
2. 停止会提交新工作的计划与排队生产者。
3. 停止 Desk Foreman manager 与服务，然后确认每个任务、会话、容器和原生子进程
   确实停止。只停 manager 可能留下 Docker 容器或原生子进程。
4. 盘点每个受管进程与容器的所有权、卷、状态和停止确认。不得影响无关容器。
5. 保持数据库可用，并阻止重启、自动修复或调度准入，直到所有检查完成。

任何未确认的停止或未解决的外部写入都会阻止处置。不确定的外部状态只能只读
对账并保留回执。

## 选择、归档与删除规则

- 处置前保守归档，尤其是有交付物、审批或关联价值的记录。
- 不整库/整 schema 清除，不 `CASCADE`/`TRUNCATE` 扫描，不做 Docker 全局 prune，
  不按通配符选择表或路径，也不删除未知卷。
- 处置历史会话不等于授权清除认证会话。

Desk Foreman 依赖处理（在选择获批后）：

1. task/job/environment 环没有简单的子先序。先清除所选行的
   task/environment 引用列（`execution_environment_tasks.prepare_job_id`、
   `start_job_id`、`stop_job_id`；`execution_environment_jobs.native_task_id`；
   `execution_environments.active_native_task_id`），再确认没有未选行仍引用所选行。
2. 子先序删除导出子树：`execution_environment_task_export_files` 与
   `execution_environment_task_export_missing_paths`，再
   `execution_environment_task_export_selection_files`，再
   `execution_environment_task_exports`。
3. 删除 `execution_environment_task_seed_files`，再
   `execution_environment_task_seeds`。
4. 删除 `execution_environment_tasks`，再 `execution_environment_jobs`，再
   `execution_environments`。
5. `audit_log` 以 NO ACTION 引用 `workspace_bindings` 并阻止删除绑定。不得通过
   置空或删除审计行绕过：归档审计证据并保留被引用绑定，直到有单独批准的保留
   处置。保留的惰性历史证据不再阻止绑定或 manager 清理。

## 重启与验证

1. 在批准的清理与 schema/版本核验后，以现有身份和私有配置先启动 Desk Foreman
   服务，再启动 runner manager。
2. 验证健康与能力端点，然后只有在新的已认证 MCP 绑定/会话及用户/工作区隔离
   冒烟检查通过后才重新开放入口。
3. 清除集断言仅限已批准选择。
4. 新工作区获得新的受支持绑定与会话。不得接管或重放旧队列任务或旧请求身份。
5. 保留通用 Local Git 能力。不得在本手册中 fetch、pull 或覆盖受支持的脏活动
   工作区。

## 失败处理

出现任何意外 schema 状态、停止状态或错误时，保持准入关闭并保留证据。备份恢复
是单独批准、协调的维护操作；不得把恢复的旧主体状态与新进程混用，也不得重放
不确定的外部写入。不存在自动回滚或恢复机制。

## 待办运维事项

- 确切的生产根目录、卷与挂载拓扑。
- 通过只读目标内省得到的确切外键删除顺序。
- 用户交付物的归档与删除处置。

生产检查、清理执行与运行时验证分别需要单独授权。
