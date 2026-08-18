import {
  createApplication,
  createApplicationToken,
  createMcpToken,
  createUser,
  getApprovalSettings,
  testApprovalSettings,
  testApplicationApproval,
  deleteApplicationToken,
  deleteMcpToken,
  deleteUser,
  listApplicationTokens,
  listApplications,
  listMcpTokens,
  listUsers,
  listWorkspaceBindings,
  listAuditLogs,
  listRunnerSessions,
  listWorkspaceRunners,
  operationsSummary,
  archiveWorkspaceBinding,
  resetWorkspaceBinding,
  restoreWorkspaceBinding,
  updateApplicationToken,
  updateMcpToken,
  resetUserPassword,
  updateApplication,
  updateApprovalSettings,
  listRunnerManagers,
  createRunnerManager,
  updateRunnerManager,
  updateUser,
  type ApplicationResponse,
  type ApplicationTokenResponse,
  type CreateApplicationRequest,
  type CreateApplicationTokenRequest,
  type CreateApplicationTokenResponse,
  type CreateMcpTokenRequest,
  type CreateMcpTokenResponse,
  type CreateUserRequest,
  type ListWorkspaceBindingsData,
  type McpTokenResponse,
  type ResetPasswordRequest,
  type UpdateUserRequest,
  type UpdateApplicationRequest,
  type UserPageResponse,
  type UserResponse,
  type WorkspaceBindingResponse,
  type AuditLogPageResponse,
  type ApprovalSettingsResponse,
  type ApprovalTestResponse,
  type ListAuditLogsData,
  type OperationsSummary,
  type RunnerSessionResponse,
  type WorkspaceRunnerResponse,
  type UpdateApplicationTokenRequest,
  type UpdateMcpTokenRequest,
  type UpdateApprovalSettingsRequest,
  type RunnerManagerResponse,
  type CreateRunnerManagerRequest,
  type CreateRunnerManagerResponse,
  type UpdateRunnerManagerRequest,
} from './generated'
import { requireOk } from './http'

export async function listAdminUsers(query: {
  limit: number
  offset: number
  sort_by?: string
  sort_dir?: 'asc' | 'desc'
}): Promise<UserPageResponse> {
  const { data, response } = await listUsers({ query })
  await requireOk(response, 'Failed to load users')
  return (
    data ?? { items: [], total: 0, limit: query.limit, offset: query.offset }
  )
}

export async function listAdminRunnerManagers(): Promise<
  RunnerManagerResponse[]
> {
  const { data, response } = await listRunnerManagers()
  await requireOk(response, 'Failed to load runner managers')
  return data ?? []
}

export async function createAdminRunnerManager(
  body: CreateRunnerManagerRequest,
): Promise<CreateRunnerManagerResponse> {
  const { data, response } = await createRunnerManager({ body })
  await requireOk(response, 'Failed to create runner manager')
  if (!data) throw new Error('Failed to create runner manager')
  return data
}

export async function updateAdminRunnerManager(
  runner_manager_id: number,
  body: UpdateRunnerManagerRequest,
): Promise<RunnerManagerResponse> {
  const { data, response } = await updateRunnerManager({
    path: { runner_manager_id },
    body,
  })
  await requireOk(response, 'Failed to update runner manager')
  if (!data) throw new Error('Failed to update runner manager')
  return data
}

export async function createAdminUser(
  body: CreateUserRequest,
): Promise<UserResponse> {
  const { data, response } = await createUser({ body })
  await requireOk(response, 'Failed to create user')
  if (!data) throw new Error('Failed to create user')
  return data
}

export async function updateAdminUser(
  user_id: number,
  body: UpdateUserRequest,
): Promise<UserResponse> {
  const { data, response } = await updateUser({ path: { user_id }, body })
  await requireOk(response, 'Failed to update user')
  if (!data) throw new Error('Failed to update user')
  return data
}

export async function resetAdminUserPassword(
  user_id: number,
  body: ResetPasswordRequest,
): Promise<void> {
  const { response } = await resetUserPassword({ path: { user_id }, body })
  await requireOk(response, 'Failed to reset password')
}

export async function deactivateAdminUser(user_id: number): Promise<void> {
  const { response } = await deleteUser({ path: { user_id } })
  await requireOk(response, 'Failed to deactivate user')
}

export async function listAdminMcpTokens(): Promise<McpTokenResponse[]> {
  const { data, response } = await listMcpTokens()
  await requireOk(response, 'Failed to load MCP tokens')
  return data ?? []
}

export async function createAdminMcpToken(
  body: CreateMcpTokenRequest,
): Promise<CreateMcpTokenResponse> {
  const { data, response } = await createMcpToken({ body })
  await requireOk(response, 'Failed to create MCP token')
  if (!data) throw new Error('Failed to create MCP token')
  return data
}

export async function deleteAdminMcpToken(token_id: number): Promise<void> {
  const { response } = await deleteMcpToken({ path: { token_id } })
  await requireOk(response, 'Failed to revoke MCP token')
}

export async function listAdminApplications(): Promise<ApplicationResponse[]> {
  const { data, response } = await listApplications()
  await requireOk(response, 'Failed to load applications')
  return data ?? []
}

export async function createAdminApplication(
  body: CreateApplicationRequest,
): Promise<ApplicationResponse> {
  const { data, response } = await createApplication({ body })
  await requireOk(response, 'Failed to create application')
  if (!data) throw new Error('Failed to create application')
  return data
}

export async function updateAdminApplication(
  application_id: number,
  body: UpdateApplicationRequest,
): Promise<ApplicationResponse> {
  const { data, response } = await updateApplication({
    path: { application_id },
    body,
  })
  await requireOk(response, 'Failed to update application')
  if (!data) throw new Error('Failed to update application')
  return data
}

export async function getAdminApprovalSettings(): Promise<ApprovalSettingsResponse> {
  const { data, response } = await getApprovalSettings()
  await requireOk(response, 'Failed to load approval settings')
  if (!data) throw new Error('Failed to load approval settings')
  return data
}

export async function updateAdminApprovalSettings(
  body: UpdateApprovalSettingsRequest,
): Promise<ApprovalSettingsResponse> {
  const { data, response } = await updateApprovalSettings({ body })
  await requireOk(response, 'Failed to update approval settings')
  if (!data) throw new Error('Failed to update approval settings')
  return data
}

export async function testAdminApprovalSettings(): Promise<ApprovalTestResponse> {
  const { data, response } = await testApprovalSettings()
  await requireOk(response, 'Failed to test approval reviewer')
  if (!data) throw new Error('Failed to test approval reviewer')
  return data
}

export async function testAdminApplicationApproval(
  application_id: number,
): Promise<ApprovalTestResponse> {
  const { data, response } = await testApplicationApproval({
    path: { application_id },
  })
  await requireOk(response, 'Failed to test application reviewer')
  if (!data) throw new Error('Failed to test application reviewer')
  return data
}

export async function listAdminApplicationTokens(): Promise<
  ApplicationTokenResponse[]
> {
  const { data, response } = await listApplicationTokens()
  await requireOk(response, 'Failed to load application tokens')
  return data ?? []
}

export async function createAdminApplicationToken(
  body: CreateApplicationTokenRequest,
): Promise<CreateApplicationTokenResponse> {
  const { data, response } = await createApplicationToken({ body })
  await requireOk(response, 'Failed to create application token')
  if (!data) throw new Error('Failed to create application token')
  return data
}

export async function deleteAdminApplicationToken(
  token_id: number,
): Promise<void> {
  const { response } = await deleteApplicationToken({ path: { token_id } })
  await requireOk(response, 'Failed to revoke application token')
}

export async function updateAdminApplicationToken(
  token_id: number,
  body: UpdateApplicationTokenRequest,
): Promise<ApplicationTokenResponse> {
  const { data, response } = await updateApplicationToken({
    path: { token_id },
    body,
  })
  await requireOk(response, 'Failed to update application token')
  if (!data) throw new Error('Failed to update application token')
  return data
}

export async function updateAdminMcpToken(
  token_id: number,
  body: UpdateMcpTokenRequest,
): Promise<McpTokenResponse> {
  const { data, response } = await updateMcpToken({ path: { token_id }, body })
  await requireOk(response, 'Failed to update MCP token')
  if (!data) throw new Error('Failed to update MCP token')
  return data
}

export async function listAdminWorkspaceBindings(
  query: ListWorkspaceBindingsData['query'],
): Promise<WorkspaceBindingResponse[]> {
  const { data, response } = await listWorkspaceBindings({ query })
  await requireOk(response, 'Failed to load workspace bindings')
  return data ?? []
}

export async function transitionAdminWorkspaceBinding(
  binding_id: number,
  action: 'archive' | 'restore' | 'reset',
): Promise<WorkspaceBindingResponse> {
  const operation =
    action === 'archive'
      ? archiveWorkspaceBinding
      : action === 'restore'
        ? restoreWorkspaceBinding
        : resetWorkspaceBinding
  const { data, response } = await operation({ path: { binding_id } })
  await requireOk(response, `Failed to ${action} workspace`)
  if (!data) throw new Error(`Failed to ${action} workspace`)
  return data
}

export async function listAdminAuditLogs(
  query: ListAuditLogsData['query'] = {},
): Promise<AuditLogPageResponse> {
  const { data, response } = await listAuditLogs({ query })
  await requireOk(response, 'Failed to load audit logs')
  return (
    data ?? {
      items: [],
      total: 0,
      limit: query.limit ?? 50,
      offset: query.offset ?? 0,
    }
  )
}

export async function listAdminRunnerSessions(): Promise<
  RunnerSessionResponse[]
> {
  const { data, response } = await listRunnerSessions()
  await requireOk(response, 'Failed to load runner sessions')
  return data ?? []
}

export async function listAdminWorkspaceRunners(): Promise<
  WorkspaceRunnerResponse[]
> {
  const { data, response } = await listWorkspaceRunners()
  await requireOk(response, 'Failed to load workspace runners')
  return data ?? []
}

export async function getAdminOperationsSummary(): Promise<OperationsSummary> {
  const { data, response } = await operationsSummary()
  await requireOk(response, 'Failed to load operations summary')
  if (!data) throw new Error('Failed to load operations summary')
  return data
}
