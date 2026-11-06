# 离线干净部署

[English](offline-clean-deploy.md) | 简体中文

本手册描述一种单实例维护部署：停止准入与执行主体，丢弃经明确批准的工作区与
历史状态，然后以干净状态启动。它仅为文档：不授权执行，也不提供可直接复制的
破坏性命令。

## 警告与批准门槛

清理不可逆。在操作者具备以下条件前不得开始：

- 一份书面批准，明确目标实例以及要丢弃的确切状态；
- 当前数据库备份及恢复决策；
- 针对所选状态的实时清单（见下文），而不仅是本文档。

本文档不是经过生产验证的清单。所有部署相关取值都必须来自实际部署，绝不能
照搬示例。

## 部署方式

- 部署采用单实例离线切换；不提供在线重置、滚动兼容、旧状态接管或恢复路径。
- 丢弃范围是下文列出的受管工作区内容及历史会话/任务历史；相关许可记录保留在
  issue 记录中。
- 登录会话不属于工作区历史，始终受保护。
- 未知或无法分类的状态一律保留，不得删除。

## 状态分类

### A. 已批准的可丢弃范围

此范围涵盖受管工作区目录，以及下文列为可清理候选的历史会话/任务历史，但仍
须遵守本手册的选择与依赖规则。它不是盲目清除范围：确切的行、路径、卷和表仍
必须逐一选择并核验。

### B. 需要按部署逐项选择的候选表

不得将其视为可直接执行的删除清单。每张表都需要明确选择、行数、保留处置和
依赖核验。

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

Stock 可清理的会话历史候选（模式 `stock_app`）：

- `ai_conversation`
- `ai_message`
- `ai_attachment`
- `ai_artifact`（可能包含用户交付物；除非所有者批准丢弃，否则应归档而非删除）
- `ai_tool_output`
- `ai_run`
- `ai_run_event`
- `ai_run_message`
- `ai_prompt_queue`
- `ai_pending_question`
- `ai_conversation_folder`
- `ai_conversation_memory_snapshot`
- `ai_memory_entry`

在丢弃历史前，必须先静默并分类 `ai_run` 与 `ai_prompt_queue` 中 pending 或
running 的工作。已入队并不证明不会再产生外部副作用。

### C. HOLD：外部写入、审批或审计证据

这些记录只可保留、归档或核对，不得纳入批量删除。保留标识与关联关系不等同于
删除证据。

Desk Foreman：

- `audit_log`
- `workspace_git_jobs`

Stock：

- `code_project_push_evidence`
- `code_project_push_evidence_ref`
- `code_project_run_authority`（必须保留；其 `workspace_binding_id` 可为空，
  用于承载外部写入证据且没有外键）
- `code_project_mutation_intent`
- `code_project_native_task_input`
- `ai_tool_approval_policy`
- `ai_tool_pending_approval`
- `ai_tool_approval_grant`
- `auth_audit_log`
- 处于 `pending`、`active` 或 `stopping` 的 `code_project_mcp_workspace_binding`：
  必须先核对，只有被证实为 `stopped` 且可丢弃的关联才是清理候选。

与所丢弃状态相关的审批、推送回执、认证审计或核对证据也必须保留。

### D. 受保护：本手册绝不删除

Desk Foreman：

- `users`
- `web_sessions`
- `mcp_tokens`
- `applications`
- `application_tokens`
- `runner_managers`

Stock：

- `users`
- `auth_passkeys`
- `stock_app`、`stock_analytics`、`stock_raw` 中的市场数据
- `code_project`
- `code_project_run`
- `code_project_runtime_config`
- `code_project_member`
- `ai_provider_endpoint`
- `ai_model_config`
- `ai_mcp_server`
- `ai_mcp_tools`
- `ai_research_preference`
- `ai_runtime_config`

不得为了满足外键而初始化或清空整个业务数据库。

## 必需的实时清单

任何删除前记录并审阅：

1. 目标实例、运行修订、数据库标识、备份引用和指定的维护负责人。
2. 所选表及其行数、保留处置，以及每项选择经检查的外键图。
3. 确切解析后的宿主机与容器路径、卷名称、挂载拓扑，以及当前任务归属和是否与
   无关项目共享的检查结果。
4. 部署中 `WORKSPACE_ROOT`、`RUNNER_HOST_WORKSPACE_ROOT`、
   `RUNNER_EXECUTION_ENVIRONMENT_STATE_DIR`、`RUNNER_NATIVE_TASK_ROOT`、
   `RUNNER_NATIVE_TASK_HOST_ROOT`、`RUNNER_GIT_PRIVATE_ROOT`、
   `RUNNER_GIT_HOST_PRIVATE_ROOT` 的实际取值，以及每个环境 UUID 对应的卷。
5. 实际使用的 Stock compose 项目/配置名称。`stock-code-projects` 与
   `stock-ai-workspaces` 只是逻辑名称；必须记录部署中的真实标识。
6. 选中的工作区路径。根目录可能是私有凭据或无关项目的父目录：绝不能凭推断
   删除挂载父目录，只能删除选中的工作区目录。

凭据归档不得进入工作区、模型流量或 Shell 输出。机器管理的凭据应通过受支持的
服务生命周期轮换或保留；绝不要把秘密复制到命令参数或临时文件中。

## 停机

1. 首先关闭外部入口与准入。
2. 停止计划任务与队列生产者，包括可能提交新工作的 Stock 编排。
3. 在停止 Desk Foreman manager 与 service 之前，先静默 Stock 消费方
   （`stock`、`stock-rust-tools`）。
4. 停止 Desk Foreman manager 与 service，然后确认所有任务、会话、容器和原生
   子进程确实已停止。仅停止 manager 可能仍留下 Docker 容器或原生子进程。
5. 盘点每个受管进程与容器及其归属、卷、状态和停止确认。不得影响无关容器。
6. 保持数据库可用，并在所有检查完成前阻止重启、autoheal 或调度准入。

任何未确认的停止或未解决的外部写入都会阻塞丢弃。绝不要盲目重放不确定的
Forgejo 或 SFTP 写入；保留回执并以只读方式核对外部状态。上述停启顺序只是
源码中的示例拓扑，不是生产部署命令。

## 选择、归档与删除规则

- 丢弃前保守归档，尤其是 `ai_artifact` 中的用户交付物以及具有审批或关联价值
  的记录。
- 不进行整库或整模式清空，不做 `CASCADE`/`TRUNCATE` 扫除，不做 Docker 全局
  清理，不使用通配表或路径，也不删除未知卷。
- 历史会话的丢弃绝不授权清除认证会话。

Desk Foreman 依赖处理（须先获得明确选择批准）：

1. 任务/作业/环境之间存在无简单子优先顺序的循环引用。在任何删除前，先对每个
   选中行清空五个任务/环境引用列（`execution_environment_tasks.prepare_job_id`、
   `start_job_id`、`stop_job_id`；`execution_environment_jobs.native_task_id`；
   `execution_environments.active_native_task_id`），然后确认没有未选中行仍引用
   选中行。
2. 按子优先顺序删除导出子树：
   `execution_environment_task_export_files` 与
   `execution_environment_task_export_missing_paths`，然后
   `execution_environment_task_export_selection_files`，然后
   `execution_environment_task_exports`。
3. 删除 `execution_environment_task_seed_files`，然后
   `execution_environment_task_seeds`。
4. 删除 `execution_environment_tasks`，然后
   `execution_environment_jobs`，最后删除 `execution_environments`。
5. `audit_log` 以 NO ACTION 引用 `workspace_bindings`，会阻塞绑定删除。不得
   通过清空或删除审计行来绕过：应归档审计证据，并保留被引用的绑定，直到存在
   另行批准的保留处置。引用绑定的 HOLD Git 作业与回执同理。
6. `workspace_git_jobs` 还会阻塞 `runner_managers` 及其绑定。删除
   `workspace_bindings` 行会通过单列与复合 owner 外键级联删除其
   `workspace_runners` 行；另外，删除 `runner_managers` 行只会把
   `workspace_runners.runner_manager_id` 置空，不会删除 runner 行。
   `runner_owner_stop_fence` 与 `runner_principal_admission` 没有外键。

Stock 各表需要各自的子优先顺序。本文不枚举具体顺序，必须在准备任何删除事务
前对目标数据库做只读外键检查后得出。遇到未知约束即停止；绝不臆造顺序，也不
为让删除成功而静默清空记录。

## 重启与核验

1. 在批准的清理与模式/版本核验完成后，使用既有身份和私有配置启动 Desk
   Foreman service，然后启动 runner manager。
2. 核验健康与能力端点。
3. 启动 Stock；只有在全新的已认证 MCP 绑定/会话以及用户/工作区隔离冒烟检查
   通过后，才重新开放入口。
4. 清除集合断言仅限已批准的选择。当保留回执仍引用 HOLD Git 作业或绑定时，
   不得要求它们为空。
5. 新工作区使用新的受支持绑定与会话。绝不接管或重放旧的排队任务或旧请求身份。
6. 保留完整 Git 能力。本手册不包含对受支持的脏工作区执行 fetch、pull 或覆盖。

## 失败处理

遇到任何意外的模式状态、停止状态或错误时，保持准入关闭并保留证据。备份恢复
属于另行批准的协调维护操作；不得把恢复的旧执行主体状态与新进程混用，也不得
重放不确定的外部写入。系统没有自动回滚或自动恢复机制。

## 待操作者确认事项

- 确切的生产根目录、卷与挂载拓扑。
- 通过目标数据库只读检查得出的 Stock 精确外键删除顺序。
- `workspace_git_jobs` 中 pending/claimed 行以及
  `code_project_mcp_workspace_binding` 中 pending/active 行的分类。
- `ai_artifact` 采用归档还是删除。

生产检查、清理执行与运行时核验在实施前均须分别获得授权。
