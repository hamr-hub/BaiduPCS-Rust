<template>
  <div class="cloud-sync-view">
    <!-- 顶部工具栏 -->
    <div class="toolbar">
      <div class="header-left">
        <h2 v-if="!isMobile">云同步</h2>
        <span class="subtitle">百度网盘 · S3 · 阿里云 OSS 互通</span>
      </div>
      <div class="header-right">
        <el-button type="primary" :icon="Plus" @click="openCreateConnection">
          新建连接
        </el-button>
        <el-button type="success" :icon="Promotion" @click="openCreateJob">
          创建同步任务
        </el-button>
        <el-button :icon="Refresh" @click="refreshAll" :loading="loading">刷新</el-button>
      </div>
    </div>

    <el-tabs v-model="activeTab" class="cs-tabs">
      <!-- ============== 连接 ============== -->
      <el-tab-pane label="存储连接" name="connections">
        <el-empty v-if="connections.length === 0" description="还没有存储连接">
          <el-button type="primary" @click="openCreateConnection">创建第一个连接</el-button>
        </el-empty>

        <div v-else class="conn-grid">
          <el-card
              v-for="c in connections"
              :key="c.id"
              class="conn-card"
              shadow="hover"
          >
            <div class="conn-header">
              <el-icon :size="20" :color="kindColor(c.config.kind)">
                <component :is="kindIcon(c.config.kind)" />
              </el-icon>
              <span class="conn-name">{{ c.config.name }}</span>
              <el-tag size="small" :type="kindTag(c.config.kind)">
                {{ kindLabel(c.config.kind) }}
              </el-tag>
            </div>
            <div class="conn-detail">
              <template v-if="c.config.kind === 'baidu'">
                <div>账号 UID: <b>{{ c.config.owner_uid }}</b></div>
              </template>
              <template v-else>
                <div>桶名: <b>{{ c.config.bucket }}</b></div>
                <div>区域: <b>{{ c.config.region }}</b></div>
                <div v-if="c.config.endpoint">端点: {{ c.config.endpoint }}</div>
              </template>
            </div>
            <div class="conn-actions">
              <el-button size="small" :icon="ConnectionIcon" @click="testConn(c)" :loading="testingId === c.id">
                测试
              </el-button>
              <el-button size="small" :icon="Delete" type="danger" @click="deleteConn(c)">
                删除
              </el-button>
            </div>
          </el-card>
        </div>
      </el-tab-pane>

      <!-- ============== 任务 ============== -->
      <el-tab-pane label="同步任务" name="jobs">
        <el-table :data="jobs" stripe v-loading="loading" empty-text="暂无任务">
          <el-table-column prop="name" label="任务名" min-width="160" />
          <el-table-column label="方向" width="100">
            <template #default="{ row }">
              <el-tag size="small" :type="row.direction === 'upload' ? 'success' : 'warning'">
                {{ directionLabel(row.direction) }}
              </el-tag>
            </template>
          </el-table-column>
          <el-table-column label="源 → 目标" min-width="280">
            <template #default="{ row }">
              <div class="path-row">
                <span class="src-name">{{ row.source_connection_name }}</span>
                <span class="path-text">{{ row.source_path }}</span>
                <el-icon class="arrow"><Right /></el-icon>
                <span class="dst-name">{{ row.dest_connection_name }}</span>
                <span class="path-text">{{ row.dest_path }}</span>
              </div>
            </template>
          </el-table-column>
          <el-table-column label="状态" width="100">
            <template #default="{ row }">
              <el-tag size="small" :type="statusTagType(row.status)">
                {{ statusLabel(row.status) }}
              </el-tag>
            </template>
          </el-table-column>
          <el-table-column label="进度" min-width="160">
            <template #default="{ row }">
              <el-progress
                  :percentage="row.total_bytes > 0 ? Math.round(row.transferred_bytes * 100 / row.total_bytes) : 0"
                  :status="row.status === 'failed' ? 'exception' : (row.status === 'completed' ? 'success' : undefined)"
              />
            </template>
          </el-table-column>
          <el-table-column label="创建时间" width="170">
            <template #default="{ row }">
              {{ formatTime(row.created_at) }}
            </template>
          </el-table-column>
          <el-table-column label="操作" width="160" fixed="right">
            <template #default="{ row }">
              <el-button
                  v-if="row.status === 'running' || row.status === 'pending'"
                  size="small"
                  type="warning"
                  @click="cancelJob(row)"
              >
                取消
              </el-button>
              <el-button
                  size="small"
                  type="danger"
                  @click="deleteJobAction(row)"
              >
                删除
              </el-button>
            </template>
          </el-table-column>
        </el-table>
      </el-tab-pane>
    </el-tabs>

    <!-- ============== 创建连接对话框 ============== -->
    <el-dialog
        v-model="createConnDialog"
        :title="connDialogTitle"
        width="560"
        :close-on-click-modal="false"
    >
      <el-form :model="connForm" label-width="100">
        <el-form-item label="连接类型">
          <el-radio-group v-model="connForm.kind" @change="resetConnForm">
            <el-radio value="s3">AWS S3 / S3 兼容</el-radio>
            <el-radio value="oss">阿里云 OSS</el-radio>
            <el-radio value="baidu">百度网盘</el-radio>
          </el-radio-group>
        </el-form-item>

        <template v-if="connForm.kind === 'baidu'">
          <el-form-item label="名称">
            <el-input v-model="connForm.name" placeholder="我的百度网盘" />
          </el-form-item>
          <el-form-item label="账号 UID">
            <el-input v-model.number="connForm.owner_uid" type="number" placeholder="已登录的百度账号 UID" />
          </el-form-item>
        </template>

        <template v-else>
          <el-form-item label="名称">
            <el-input v-model="connForm.name" placeholder="连接名称" />
          </el-form-item>
          <el-form-item label="区域">
            <el-input v-model="connForm.region" :placeholder="connForm.kind === 'oss' ? 'oss-cn-hangzhou' : 'us-east-1'" />
          </el-form-item>
          <el-form-item label="桶名">
            <el-input v-model="connForm.bucket" placeholder="my-bucket" />
          </el-form-item>
          <el-form-item label="AccessKey">
            <el-input v-model="connForm.access_key" placeholder="AKID..." />
          </el-form-item>
          <el-form-item label="SecretKey">
            <el-input v-model="connForm.secret_key" type="password" show-password placeholder="•••" />
          </el-form-item>
          <el-form-item label="Endpoint">
            <el-input
                v-model="connForm.endpoint"
                :placeholder="connForm.kind === 'oss' ? '留空自动用 https://oss-<region>.aliyuncs.com' : '留空使用 AWS 默认; 自建/MinIO 填 http://host:port'"
            />
          </el-form-item>
          <el-form-item label="Path Style">
            <el-switch v-model="connForm.path_style" />
            <span class="hint">自建 / MinIO 通常需要开启</span>
          </el-form-item>
        </template>
      </el-form>

      <template #footer>
        <el-button @click="createConnDialog = false">取消</el-button>
        <el-button type="primary" @click="submitConn" :loading="submittingConn">
          创建
        </el-button>
      </template>
    </el-dialog>

    <!-- ============== 创建任务对话框 ============== -->
    <el-dialog
        v-model="createJobDialog"
        title="创建同步任务"
        width="640"
        :close-on-click-modal="false"
    >
      <el-form :model="jobForm" label-width="100">
        <el-form-item label="任务名">
          <el-input v-model="jobForm.name" placeholder="例: 备份图片到 S3" />
        </el-form-item>
        <el-form-item label="源连接">
          <el-select v-model="jobForm.source_connection_id" placeholder="选择源" style="width: 100%">
            <el-option
                v-for="c in connections"
                :key="c.id"
                :value="c.id"
                :label="`${c.config.name} (${kindLabel(c.config.kind)})`"
            />
          </el-select>
        </el-form-item>
        <el-form-item label="源路径">
          <el-input
              v-model="jobForm.source_path"
              :placeholder="jobForm.source_connection_id && getConnKind(jobForm.source_connection_id) ? pathHint(getConnKind(jobForm.source_connection_id) as StorageKind, 'src') : '先选源连接'"
          />
        </el-form-item>
        <el-form-item label="目标连接">
          <el-select v-model="jobForm.dest_connection_id" placeholder="选择目标" style="width: 100%">
            <el-option
                v-for="c in connections"
                :key="c.id"
                :value="c.id"
                :label="`${c.config.name} (${kindLabel(c.config.kind)})`"
            />
          </el-select>
        </el-form-item>
        <el-form-item label="目标路径">
          <el-input
              v-model="jobForm.dest_path"
              :placeholder="jobForm.dest_connection_id && getConnKind(jobForm.dest_connection_id) ? pathHint(getConnKind(jobForm.dest_connection_id) as StorageKind, 'dst') : '先选目标连接'"
          />
        </el-form-item>
        <el-alert
            v-if="directionPreview"
            :title="directionPreview"
            type="info"
            :closable="false"
            show-icon
        />
      </el-form>

      <template #footer>
        <el-button @click="createJobDialog = false">取消</el-button>
        <el-button type="primary" @click="submitJob" :loading="submittingJob">
          创建并执行
        </el-button>
      </template>
    </el-dialog>
  </div>
</template>

<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, markRaw } from 'vue'
import { ElMessage, ElMessageBox } from 'element-plus'
import {
  Plus,
  Refresh,
  Delete,
  Connection as ConnectionIcon,
  Promotion,
  Right,
  FolderOpened,
  Coin,
  Cloudy,
} from '@element-plus/icons-vue'
import { useIsMobile } from '@/utils/responsive'
import {
  listConnections,
  createConnection,
  deleteConnection,
  testConnection,
  listJobs,
  createJob,
  deleteJob,
  cancelJob as cancelJobApi,
  KIND_LABEL,
  STATUS_LABEL,
  STATUS_TAG_TYPE,
  DIRECTION_LABEL,
  type Connection,
  type CreateJobRequest,
  type StorageKind,
  type SyncDirection,
  type JobStatus,
} from '@/api/cloudSync'

const isMobile = useIsMobile()
const loading = ref(false)
const testingId = ref<string | null>(null)
const activeTab = ref('connections')

const connections = ref<Connection[]>([])
const jobs = ref<any[]>([])

// ============== 创建连接 ==============
const createConnDialog = ref(false)
const submittingConn = ref(false)
const connForm = ref<any>({
  kind: 's3',
  name: '',
  region: '',
  bucket: '',
  access_key: '',
  secret_key: '',
  endpoint: '',
  path_style: false,
  owner_uid: 0,
})
const connDialogTitle = computed(() => '新建存储连接')

function resetConnForm() {
  connForm.value = {
    kind: connForm.value.kind,
    name: '',
    region: '',
    bucket: '',
    access_key: '',
    secret_key: '',
    endpoint: '',
    path_style: false,
    owner_uid: 0,
  }
}

function openCreateConnection() {
  resetConnForm()
  createConnDialog.value = true
}

async function submitConn() {
  if (!connForm.value.name) {
    ElMessage.warning('请填写名称')
    return
  }
  if (connForm.value.kind === 'baidu') {
    if (!connForm.value.owner_uid) {
      ElMessage.warning('请填写账号 UID')
      return
    }
  } else {
    if (!connForm.value.region || !connForm.value.bucket || !connForm.value.access_key || !connForm.value.secret_key) {
      ElMessage.warning('请完整填写连接信息')
      return
    }
  }
  submittingConn.value = true
  try {
    const payload: any = { kind: connForm.value.kind }
    if (connForm.value.kind === 'baidu') {
      payload.name = connForm.value.name
      payload.owner_uid = Number(connForm.value.owner_uid)
    } else {
      Object.assign(payload, {
        name: connForm.value.name,
        region: connForm.value.region,
        bucket: connForm.value.bucket,
        access_key: connForm.value.access_key,
        secret_key: connForm.value.secret_key,
      })
      if (connForm.value.endpoint) payload.endpoint = connForm.value.endpoint
      if (connForm.value.path_style) payload.path_style = true
    }
    await createConnection(payload)
    ElMessage.success('连接已创建')
    createConnDialog.value = false
    await loadConnections()
  } catch (e: any) {
    ElMessage.error(e?.message || '创建失败')
  } finally {
    submittingConn.value = false
  }
}

async function testConn(c: Connection) {
  testingId.value = c.id
  try {
    const r = await testConnection(c.id)
    if (r.ok) {
      ElMessage.success(`连接成功 · 延迟 ${r.latency_ms}ms · 示例对象 ${r.sample_objects.length} 个`)
    } else {
      ElMessage.error(`连接失败: ${r.error || '未知错误'}`)
    }
  } catch (e: any) {
    ElMessage.error(e?.message || '测试失败')
  } finally {
    testingId.value = null
  }
}

async function deleteConn(c: Connection) {
  try {
    await ElMessageBox.confirm(`确认删除连接「${c.config.name}」？`, '删除连接', { type: 'warning' })
    await deleteConnection(c.id)
    ElMessage.success('已删除')
    await loadConnections()
  } catch (e: any) {
    if (e !== 'cancel') ElMessage.error(e?.message || '删除失败')
  }
}

// ============== 创建任务 ==============
const createJobDialog = ref(false)
const submittingJob = ref(false)
const jobForm = ref<CreateJobRequest>({
  name: '',
  source_connection_id: '',
  dest_connection_id: '',
  source_path: '',
  dest_path: '',
})

function openCreateJob() {
  if (connections.value.length < 2) {
    ElMessage.warning('至少需要 2 个连接才能创建同步任务')
    return
  }
  jobForm.value = {
    name: '',
    source_connection_id: '',
    dest_connection_id: '',
    source_path: '',
    dest_path: '',
  }
  createJobDialog.value = true
}

const directionPreview = computed(() => {
  if (!jobForm.value.source_connection_id || !jobForm.value.dest_connection_id) return ''
  const srcKind = getConnKind(jobForm.value.source_connection_id)
  const dstKind = getConnKind(jobForm.value.dest_connection_id)
  if (!srcKind || !dstKind) return ''
  if (srcKind === 'baidu') return `方向: 下载 · 百度 → ${kindLabel(dstKind)}`
  if (dstKind === 'baidu') return `方向: 上传 · ${kindLabel(srcKind)} → 百度`
  return `方向: 跨对象存储互通 · ${kindLabel(srcKind)} ↔ ${kindLabel(dstKind)}`
})

async function submitJob() {
  if (!jobForm.value.name) return ElMessage.warning('请填写任务名')
  if (!jobForm.value.source_connection_id) return ElMessage.warning('请选择源连接')
  if (!jobForm.value.dest_connection_id) return ElMessage.warning('请选择目标连接')
  if (!jobForm.value.source_path) return ElMessage.warning('请填写源路径')
  if (!jobForm.value.dest_path) return ElMessage.warning('请填写目标路径')
  if (jobForm.value.source_connection_id === jobForm.value.dest_connection_id) {
    return ElMessage.warning('源和目标不能是同一个连接')
  }
  submittingJob.value = true
  try {
    const job = await createJob(jobForm.value)
    ElMessage.success(`任务已创建 · ${job.name}`)
    createJobDialog.value = false
    await loadJobs()
    activeTab.value = 'jobs'
  } catch (e: any) {
    ElMessage.error(e?.message || '创建失败')
  } finally {
    submittingJob.value = false
  }
}

async function cancelJob(row: any) {
  try {
    await cancelJobApi(row.id)
    ElMessage.success('已取消')
    await loadJobs()
  } catch (e: any) {
    ElMessage.error(e?.message || '取消失败')
  }
}

async function deleteJobAction(row: any) {
  try {
    await ElMessageBox.confirm(`确认删除任务「${row.name}」？`, '删除任务', { type: 'warning' })
    await deleteJob(row.id)
    ElMessage.success('已删除')
    await loadJobs()
  } catch (e: any) {
    if (e !== 'cancel') ElMessage.error(e?.message || '删除失败')
  }
}

// ============== 加载 ==============
async function loadConnections() {
  loading.value = true
  try {
    connections.value = await listConnections()
  } catch (e: any) {
    ElMessage.error(e?.message || '加载连接失败')
  } finally {
    loading.value = false
  }
}

async function loadJobs() {
  loading.value = true
  try {
    jobs.value = await listJobs()
  } catch (e: any) {
    ElMessage.error(e?.message || '加载任务失败')
  } finally {
    loading.value = false
  }
}

async function refreshAll() {
  await Promise.all([loadConnections(), loadJobs()])
}

let pollTimer: number | null = null
onMounted(async () => {
  await refreshAll()
  pollTimer = window.setInterval(async () => {
    if (activeTab.value === 'jobs') await loadJobs()
  }, 5000)
})
onUnmounted(() => {
  if (pollTimer) clearInterval(pollTimer)
})

// ============== 工具 ==============
function kindLabel(k: StorageKind): string {
  return KIND_LABEL[k]
}
function directionLabel(d: SyncDirection): string {
  return DIRECTION_LABEL[d]
}
function statusLabel(s: JobStatus): string {
  return STATUS_LABEL[s]
}
function statusTagType(s: JobStatus): string {
  return STATUS_TAG_TYPE[s]
}

function kindIcon(k: StorageKind) {
  if (k === 'baidu') return markRaw(FolderOpened)
  if (k === 'oss') return markRaw(Coin)
  return markRaw(Cloudy)
}

function kindColor(k: StorageKind): string {
  if (k === 'baidu') return '#409eff'
  if (k === 'oss') return '#ff6b35'
  return '#67c23a'
}

function kindTag(k: StorageKind): 'primary' | 'success' | 'warning' | 'info' {
  if (k === 'baidu') return 'primary'
  if (k === 'oss') return 'warning'
  return 'success'
}

function getConnKind(id: string): StorageKind | null {
  const c = connections.value.find((x: Connection) => x.id === id)
  return c ? c.config.kind : null
}

function pathHint(kind: StorageKind | null, role: 'src' | 'dst'): string {
  if (!kind) return role === 'src' ? '请先选择连接' : '请先选择连接'
  if (kind === 'baidu') return '/apps/your-app/path/to/file.txt'
  if (kind === 'oss') return role === 'src' ? 'object-key (例: backup/2026/file.zip)' : 'object-key'
  return role === 'src' ? 'object-key' : 'object-key (例: incoming/file.zip)'
}

function formatTime(iso: string): string {
  try {
    const d = new Date(iso)
    if (Number.isNaN(d.getTime())) return iso
    const pad = (n: number) => n.toString().padStart(2, '0')
    return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`
  } catch {
    return iso
  }
}
</script>

<style scoped lang="scss">
.cloud-sync-view {
  padding: 16px;
  height: 100%;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.toolbar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 16px;
  flex-shrink: 0;

  .header-left {
    display: flex;
    align-items: baseline;
    gap: 12px;
    h2 {
      margin: 0;
    }
    .subtitle {
      color: #909399;
      font-size: 13px;
    }
  }

  .header-right {
    display: flex;
    gap: 8px;
  }
}

.cs-tabs {
  flex: 1;
  overflow: hidden;
  :deep(.el-tabs__content) {
    height: calc(100% - 56px);
    overflow: auto;
  }
  :deep(.el-tab-pane) {
    height: 100%;
  }
}

.conn-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(280px, 1fr));
  gap: 16px;
}

.conn-card {
  transition: transform 0.2s;
  &:hover {
    transform: translateY(-2px);
  }
}

.conn-header {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 12px;

  .conn-name {
    font-weight: 600;
    font-size: 16px;
    flex: 1;
  }
}

.conn-detail {
  color: #606266;
  font-size: 13px;
  line-height: 1.8;
  margin-bottom: 12px;
  word-break: break-all;
}

.conn-actions {
  display: flex;
  gap: 8px;
  border-top: 1px solid #ebeef5;
  padding-top: 12px;
}

.path-row {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-wrap: wrap;
  font-size: 13px;

  .src-name,
  .dst-name {
    font-weight: 600;
    color: #303133;
  }
  .path-text {
    color: #606266;
    font-family: monospace;
  }
  .arrow {
    color: #909399;
    margin: 0 4px;
  }
}

.hint {
  margin-left: 12px;
  color: #909399;
  font-size: 12px;
}
</style>