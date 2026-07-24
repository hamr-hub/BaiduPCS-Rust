<template>
  <div class="share-include-exclude-editor">
    <!-- 同步路径（include_paths） -->
    <div class="field-block">
      <div class="field-label">同步路径</div>
      <div class="path-editor">
        <div v-if="includePaths.length === 0" class="path-empty">
          不选则同步整个分享；点「从分享浏览」可一次勾选多个文件/文件夹——勾选文件夹＝同步其中全部内容，勾选文件＝只同步该文件。
        </div>
        <div v-else class="path-tags">
          <el-tag
              v-for="(p, i) in includePaths"
              :key="p + i"
              closable
              @close="removeInclude(i)"
              style="margin: 2px 4px 2px 0"
          >{{ p }}</el-tag>
        </div>
        <div class="path-actions">
          <el-tooltip
              :disabled="ownerLoggedIn !== false"
              content="订阅所属账号未登录，请先登录该账号再预览目录树"
              placement="top"
          >
            <span>
              <el-button
                  size="small"
                  :icon="FolderOpened"
                  :loading="previewing"
                  :disabled="!shareUrl || ownerLoggedIn === false"
                  @click="openPicker"
              >从分享浏览</el-button>
            </span>
          </el-tooltip>
        </div>
        <div v-if="ownerLoggedIn === false" class="owner-offline-hint">
          订阅所属账号未登录，目录树预览不可用，请先登录该账号
        </div>
      </div>
    </div>

    <!-- 排除规则（exclude_patterns） -->
    <div class="field-block">
      <div class="field-label">排除规则</div>
      <div class="path-editor">
        <div class="path-tags">
          <el-tag
              v-for="(p, i) in excludePatterns"
              :key="p + i"
              closable
              type="info"
              @close="removeExclude(i)"
              style="margin: 2px 4px 2px 0"
          >{{ p }}</el-tag>
          <span v-if="excludePatterns.length === 0" class="path-empty-inline">
            支持 glob：*.tmp、sample.*、*广告* 等
          </span>
        </div>
        <div class="exclude-input-row">
          <!-- textarea：回车提交当前内容；粘贴多行文本原生保留换行 → 每行一条。
               不用逗号等标点当分隔符 —— 它们是合法文件名字符，会切坏真实规则。 -->
          <el-input
              v-model="excludeInput"
              type="textarea"
              :autosize="{ minRows: 1, maxRows: 4 }"
              resize="none"
              size="small"
              placeholder="输入规则后回车添加；可粘贴多行一次添加（每行一条）"
              style="width: 280px"
              @keydown.enter="onExcludeEnter"
          />
          <el-button size="small" plain type="primary" @click="addExclude">添加</el-button>
          <el-button size="small" text type="primary" @click="showExcludeHelp = !showExcludeHelp">
            <el-icon style="margin-right: 2px"><QuestionFilled /></el-icon>使用说明
          </el-button>
        </div>
        <!-- 常用规则词表：来自系统设置（后端配置），点击即添加到当前规则。
             词表的增删在 系统设置 → 分享同步 中维护。 -->
        <div v-if="excludePresets.length > 0" class="preset-row">
          <span class="preset-label">常用：</span>
          <el-tag
              v-for="p in excludePresets"
              :key="p"
              size="small"
              effect="plain"
              class="preset-tag"
              :type="excludePatterns.includes(p) ? 'success' : 'info'"
              @click="applyPreset(p)"
          >{{ p }}</el-tag>
          <el-button
              size="small"
              plain
              :icon="Star"
              :disabled="excludePatterns.length === 0 || savingPresets"
              :loading="savingPresets"
              @click="saveCurrentAsPresets"
          >存为常用</el-button>
          <el-button size="small" plain :icon="Setting" @click="goManagePresets">管理</el-button>
        </div>
        <div v-else class="preset-row">
          <span class="preset-label">
            常用规则词表为空，可在
            <el-link type="primary" :underline="false" style="font-size: 12px; vertical-align: baseline" @click="goManagePresets">系统设置 → 分享同步</el-link>
            中配置
          </span>
          <el-button
              size="small"
              plain
              :icon="Star"
              :disabled="excludePatterns.length === 0 || savingPresets"
              :loading="savingPresets"
              @click="saveCurrentAsPresets"
          >存为常用</el-button>
        </div>
        <ExcludeRuleHelp v-if="showExcludeHelp" />
      </div>
    </div>

    <!-- 从分享选择子路径：复用转存同款 ShareFileSelector -->
    <el-dialog
        v-model="pickerVisible"
        title="从分享中选择要同步的子路径"
        width="680px"
        :close-on-click-modal="false"
        append-to-body
    >
      <ShareFileSelector
          :files="previewFiles"
          :loading="previewing"
          :share-info="previewShareInfo"
          :share-url="shareUrl"
          :share-password="password || undefined"
          @update:selected-files="onSelectedFiles"
      />
      <template #footer>
        <span class="picker-hint">已选 {{ pendingSelected.length }} 项</span>
        <el-button @click="pickerVisible = false">取消</el-button>
        <el-button type="primary" @click="confirmPicker">确定</el-button>
      </template>
    </el-dialog>
  </div>
</template>

<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { useRouter } from 'vue-router'
import { ElMessage } from 'element-plus'
import type { AxiosError } from 'axios'
import { FolderOpened, QuestionFilled, Setting, Star } from '@element-plus/icons-vue'
import ShareFileSelector from './ShareFileSelector.vue'
import ExcludeRuleHelp from './ExcludeRuleHelp.vue'
import { getConfig, updateConfig } from '@/api/config'
import { mergeRules, parseRuleInput } from '@/utils/excludeRules'
import {
  previewShareFiles,
  type SharedFileInfo,
  type PreviewShareInfo,
} from '@/api/transfer'

const props = defineProps<{
  shareUrl: string
  password?: string | null
  includePaths: string[]
  excludePatterns: string[]
  // 订阅所属账号：编辑订阅时传订阅自己的 owner_uid，预览目录树按该账号路由 client
  // （不传则后端回退当前 active 账号——转存对话框创建场景 owner=active）
  ownerUid?: number | null
  // 订阅所属账号是否已登录：显式 false 时禁用目录树预览并提示登录该账号。
  // undefined（创建场景）= 默认按已登录处理。
  ownerLoggedIn?: boolean
}>()

const emit = defineEmits<{
  'update:includePaths': [value: string[]]
  'update:excludePatterns': [value: string[]]
}>()

const router = useRouter()
const excludeInput = ref('')

// 分享文件选择器（与转存对话框同款体验）
const pickerVisible = ref(false)
const previewing = ref(false)
const previewFiles = ref<SharedFileInfo[]>([])
const previewShareInfo = ref<PreviewShareInfo | null>(null)
const pendingSelected = ref<SharedFileInfo[]>([])

function normalizePath(v: string): string {
  const s = v.trim().replace(/\/+/g, '/')
  if (!s) return ''
  const prefixed = s.startsWith('/') ? s : `/${s}`
  if (prefixed.length === 1) return '/'
  return prefixed.endsWith('/') ? prefixed.slice(0, -1) : prefixed
}

function removeInclude(i: number) {
  const next = [...props.includePaths]
  next.splice(i, 1)
  emit('update:includePaths', next)
}

// 排除规则使用说明的展开态
const showExcludeHelp = ref(false)

/** 合并规则进当前列表并 emit（无变化不 emit） */
function emitMergedRules(incoming: string[]) {
  const next = mergeRules(props.excludePatterns, incoming)
  if (next.length !== props.excludePatterns.length) {
    emit('update:excludePatterns', next)
  }
}

/** 提交输入框内容：按行拆成规则（行内逗号等标点属于规则本身，不切分） */
function addExclude() {
  emitMergedRules(parseRuleInput(excludeInput.value))
  excludeInput.value = ''
}

/**
 * textarea 里回车 = 提交（逐条输入的自然节奏），而不是换行。
 * 中文输入法组词期间的回车（选字确认）不触发提交。
 * 粘贴多行文本无需拦截 —— textarea 原生保留换行，提交时按行拆分。
 */
function onExcludeEnter(e: Event) {
  const ke = e as KeyboardEvent
  if (ke.isComposing) return // 输入法组词中，回车是选字不是提交
  ke.preventDefault()
  addExclude()
}

// ==================== 常用规则词表 ====================
// 词表存在后端配置（系统设置 → 分享同步 中维护），跨浏览器/设备一致。
// 这里只做两件事：加载展示 + 点选添加；另提供「存为常用」快捷写回。
const excludePresets = ref<string[]>([])
const savingPresets = ref(false)

onMounted(async () => {
  try {
    const cfg = await getConfig()
    excludePresets.value = cfg.share_sync?.exclude_preset_rules ?? []
  } catch {
    // 配置加载失败不阻断编辑器；词表行为空态，不影响手输规则
  }
})

/** 点击词表 chip → 添加到当前规则（已存在则忽略） */
function applyPreset(p: string) {
  if (!props.excludePatterns.includes(p)) {
    emit('update:excludePatterns', [...props.excludePatterns, p])
  }
}

/** 把当前已填的规则并入词表并写回后端配置，方便下次建订阅时直接点选 */
async function saveCurrentAsPresets() {
  if (props.excludePatterns.length === 0 || savingPresets.value) return
  savingPresets.value = true
  try {
    // 读最新配置再合并，避免覆盖设置页刚保存的其它字段
    const cfg = await getConfig()
    const list = cfg.share_sync?.exclude_preset_rules ?? []
    const merged = [...list]
    for (const p of props.excludePatterns) {
      if (!merged.includes(p)) merged.push(p)
    }
    cfg.share_sync = { ...(cfg.share_sync ?? {}), exclude_preset_rules: merged }
    await updateConfig(cfg)
    excludePresets.value = merged
    ElMessage.success('已存为常用规则（可在系统设置中管理）')
  } catch (e) {
    ElMessage.error(`保存常用规则失败: ${(e as Error)?.message || '未知错误'}`)
  } finally {
    savingPresets.value = false
  }
}

/** 跳转到系统设置的分享同步区块管理词表（设置页支持 ?tab=xxx 定位 section） */
function goManagePresets() {
  router.push({ path: '/settings', query: { tab: 'sharesync' } })
}

function removeExclude(i: number) {
  const next = [...props.excludePatterns]
  next.splice(i, 1)
  emit('update:excludePatterns', next)
}

async function openPicker() {
  if (!props.shareUrl || !props.shareUrl.trim()) {
    ElMessage.warning('请先填写分享链接')
    return
  }
  if (props.ownerLoggedIn === false) {
    ElMessage.warning('订阅所属账号未登录，请先登录该账号再预览目录树')
    return
  }
  previewing.value = true
  try {
    const resp = await previewShareFiles({
      share_url: props.shareUrl.trim(),
      password: props.password || undefined,
    })
    previewFiles.value = resp.files || []
    previewShareInfo.value = resp.share_info ?? null
    pendingSelected.value = []
    pickerVisible.value = true
  } catch (e) {
    const ax = e as AxiosError<{ message?: string; error?: string }>
    ElMessage.error(
        ax?.response?.data?.message ||
        ax?.response?.data?.error ||
        (e as Error)?.message ||
        '加载分享文件失败',
    )
  } finally {
    previewing.value = false
  }
}

function onSelectedFiles(files: SharedFileInfo[]) {
  pendingSelected.value = files
}

function confirmPicker() {
  const selectedPaths = pendingSelected.value
      .map((f) => normalizePath(f.path))
      .filter((p) => p.length > 0)
  // 与已有路径合并去重；勾选目录即代表整棵子树，裁剪掉被祖先覆盖的冗余后代
  const merged: string[] = []
  const seen = new Set<string>()
  for (const p of [...props.includePaths, ...selectedPaths]) {
    if (seen.has(p)) continue
    seen.add(p)
    merged.push(p)
  }
  const trimmed = merged.filter(
      (p) => !merged.some((a) => a !== p && p.startsWith(`${a}/`)),
  )
  emit('update:includePaths', trimmed)
  pickerVisible.value = false
}
</script>

<style scoped lang="scss">
.field-block {
  margin-bottom: 12px;
}

.field-label {
  font-size: 13px;
  color: var(--el-text-color-regular);
  margin-bottom: 6px;
}

.path-editor {
  width: 100%;
}

.path-empty {
  font-size: 12px;
  color: var(--el-text-color-secondary);
  margin-bottom: 6px;
}

.owner-offline-hint {
  font-size: 12px;
  color: var(--el-color-warning);
  margin-top: 6px;
}

.path-empty-inline {
  font-size: 12px;
  color: var(--el-text-color-secondary);
}

.path-tags {
  margin-bottom: 6px;
}

.path-actions {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 4px;
}

.picker-hint {
  margin-right: auto;
  font-size: 12px;
  color: var(--el-text-color-secondary);
}

.exclude-input-row {
  display: flex;
  align-items: center;
  gap: 4px;
  margin-top: 4px;
}

.preset-row {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 4px;
  margin-top: 6px;
}

.preset-label {
  font-size: 12px;
  color: var(--el-text-color-secondary);
  flex-shrink: 0;
}

// 词表 chip 可点击添加，给出手型反馈
.preset-tag {
  cursor: pointer;
  user-select: none;
}

</style>
