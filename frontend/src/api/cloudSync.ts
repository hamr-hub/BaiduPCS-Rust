/**
 * 云同步 API 客户端
 *
 * 后端基址：/api/v1/cloud-sync
 * 覆盖三端互相同步：百度网盘 / S3 / Aliyun OSS
 */

import { rawApiClient } from './client'

// ==================== 类型定义 ====================

export type StorageKind = 's3' | 'oss' | 'baidu'
export type SyncDirection = 'upload' | 'download'
export type JobStatus = 'pending' | 'running' | 'completed' | 'failed' | 'cancelled'

export interface S3Config {
  name: string
  region: string
  bucket: string
  access_key: string
  secret_key: string
  endpoint?: string | null
  path_style?: boolean | null
}

export interface OssConfig {
  name: string
  region: string
  bucket: string
  access_key: string
  secret_key: string
  endpoint?: string | null
  internal_endpoint?: string | null
  path_style?: boolean | null
}

export interface BaiduConfig {
  name: string
  owner_uid: number
}

export type ConnectionConfig =
  | ({ kind: 's3' } & S3Config)
  | ({ kind: 'oss' } & OssConfig)
  | ({ kind: 'baidu' } & BaiduConfig)

export type CreateConnectionRequest = ConnectionConfig

export interface UpdateConnectionRequest {
  name?: string
  access_key?: string
  secret_key?: string
  region?: string
  bucket?: string
  endpoint?: string
  internal_endpoint?: string
  path_style?: boolean
  owner_uid?: number
}

export interface Connection {
  id: string
  config: ConnectionConfig
  created_at: string
  updated_at: string
}

export interface TransferJob {
  id: string
  name: string
  source_connection_id: string
  dest_connection_id: string
  source_path: string
  dest_path: string
  direction: SyncDirection
  status: JobStatus
  transferred_bytes: number
  total_bytes: number
  error?: string | null
  created_at: string
  updated_at: string
  finished_at?: string | null
  local_cache_path?: string | null
  owner_uid?: number | null
}

export interface JobSummary {
  id: string
  name: string
  direction: SyncDirection
  source_connection_id: string
  source_connection_name: string
  dest_connection_id: string
  dest_connection_name: string
  source_path: string
  dest_path: string
  status: JobStatus
  transferred_bytes: number
  total_bytes: number
  error?: string | null
  created_at: string
  updated_at: string
  finished_at?: string | null
}

export interface CreateJobRequest {
  name: string
  source_connection_id: string
  dest_connection_id: string
  source_path: string
  dest_path: string
  owner_uid?: number
}

export interface TestConnectionResult {
  ok: boolean
  latency_ms: number
  error?: string | null
  sample_objects: string[]
}

export interface ObjectInfo {
  key: string
  size: number
  last_modified?: string | null
  etag?: string | null
}

export interface ListObjectsResult {
  objects: ObjectInfo[]
  prefixes: string[]
  truncated: boolean
}

// ==================== Connections ====================

export async function listConnections(): Promise<Connection[]> {
  const resp = await rawApiClient.get('/cloud-sync/connections')
  return resp.data?.data ?? []
}

export async function getConnection(id: string): Promise<Connection> {
  const resp = await rawApiClient.get(`/cloud-sync/connections/${id}`)
  return resp.data?.data
}

export async function createConnection(req: CreateConnectionRequest): Promise<Connection> {
  const resp = await rawApiClient.post('/cloud-sync/connections', req)
  return resp.data?.data
}

export async function updateConnection(
  id: string,
  req: UpdateConnectionRequest,
): Promise<Connection> {
  const resp = await rawApiClient.put(`/cloud-sync/connections/${id}`, req)
  return resp.data?.data
}

export async function deleteConnection(id: string): Promise<void> {
  await rawApiClient.delete(`/cloud-sync/connections/${id}`)
}

export async function testConnection(id: string): Promise<TestConnectionResult> {
  const resp = await rawApiClient.post(`/cloud-sync/connections/${id}/test`)
  return resp.data?.data
}

export async function listConnectionObjects(
  id: string,
  prefix: string = '',
): Promise<ListObjectsResult> {
  const resp = await rawApiClient.post(
    `/cloud-sync/connections/${id}/list`,
    {},
    { params: { prefix } },
  )
  return resp.data?.data
}

// ==================== Jobs ====================

export async function listJobs(): Promise<JobSummary[]> {
  const resp = await rawApiClient.get('/cloud-sync/jobs')
  return resp.data?.data ?? []
}

export async function getJob(id: string): Promise<TransferJob> {
  const resp = await rawApiClient.get(`/cloud-sync/jobs/${id}`)
  return resp.data?.data
}

export async function createJob(req: CreateJobRequest): Promise<TransferJob> {
  const resp = await rawApiClient.post('/cloud-sync/jobs', req)
  return resp.data?.data
}

export async function deleteJob(id: string): Promise<void> {
  await rawApiClient.delete(`/cloud-sync/jobs/${id}`)
}

export async function cancelJob(id: string): Promise<void> {
  await rawApiClient.post(`/cloud-sync/jobs/${id}/cancel`)
}

// ==================== Helpers ====================

export const KIND_LABEL: Record<StorageKind, string> = {
  s3: 'AWS S3 / S3 兼容',
  oss: '阿里云 OSS',
  baidu: '百度网盘',
}

export const STATUS_LABEL: Record<JobStatus, string> = {
  pending: '等待中',
  running: '传输中',
  completed: '已完成',
  failed: '失败',
  cancelled: '已取消',
}

export const STATUS_TAG_TYPE: Record<JobStatus, string> = {
  pending: 'info',
  running: 'warning',
  completed: 'success',
  failed: 'danger',
  cancelled: '',
}

export const DIRECTION_LABEL: Record<SyncDirection, string> = {
  upload: '上传至',
  download: '下载到',
}