/**
 * 远端文件变动通知订阅 Composable
 *
 * 后端 `RecentWatcher` 在轮询时检测到**网盘根目录**新增 / mtime 变化条目，
 * 通过 WebSocket 推 `TaskEvent::System(SystemEvent::RemoteFileChanged)`。
 * 本模块订阅 `system` 分类的事件，把它们聚合成一个 in-memory 列表，
 * 暴露给 `FilesView.vue` 用来：
 *
 * 1. 在 navbar 弹"网盘有新着"小气泡
 * 2. 自动重新拉当前目录列表（最直接、最稳定，避免缓存陈旧）
 * 3. 提供 `clear()` 让用户主动消除气泡
 *
 * ⚠️ 注意 —— 远端 watch 只扫**根 `/`**，子目录里的变化不会被发现。
 * 这是已知妥协：递归会触发百度的反爬节奏限制。要更深就得用户主动挂 share-sync。
 * 前端应**不要**承诺完整盘感知，UI 文案写"根目录新文件"或"网盘新文件"。
 */

import { ref, onMounted, onUnmounted, type Ref } from 'vue'
import { getWebSocketClient, type ConnectionState } from '@/utils/websocket'
import type { SystemEvent, SystemEventRemoteFileChanged } from '@/types/events'

/** 给前端消费的"新文件条目" */
export interface RemoteFileChangeEntry extends SystemEventRemoteFileChanged {
  /** 接收时间戳（Unix 毫秒）—— 前端按 it 渲染"X 秒前到达" */
  received_at_ms: number
}

/** Composable 暴露的接口 */
export interface UseRemoteFileChangesReturn {
  /** 当前累积的远端新文件条目（只增不减，clear() 一次性清空） */
  changes: Ref<RemoteFileChangeEntry[]>
  /** 当前 WS 连接状态（Offline 时不会再来新事件） */
  connectionState: Ref<ConnectionState>
  /** 用户点过气泡后调一次：清空累积列表 */
  clear(): void
  /** 给非订阅场景准备的兜底：手动移除单个 fs_id（重命名场景下避免重复提醒） */
  dismissByFsId(fs_id: number): void
}

/** 默认累积上限 —— 超过就丢最早的，防止长会话内存爆炸 */
const MAX_RETAINED_ENTRIES = 200

export function useRemoteFileChanges(): UseRemoteFileChangesReturn {
  const changes = ref<RemoteFileChangeEntry[]>([])
  const connectionState = ref<ConnectionState>('disconnected')

  let unsubMessage: (() => void) | null = null
  let unsubState: (() => void) | null = null

  /** 推一条新事件到累积列表（FIFO + 截断） */
  function pushChange(evt: SystemEventRemoteFileChanged) {
    const entry: RemoteFileChangeEntry = {
      ...evt,
      received_at_ms: Date.now(),
    }
    changes.value.push(entry)
    // 截断到 MAX_RETAINED_ENTRIES —— 长会话 / 高活跃账号下避免内存爆炸
    if (changes.value.length > MAX_RETAINED_ENTRIES) {
      changes.value.splice(0, changes.value.length - MAX_RETAINED_ENTRIES)
    }
  }

  /** 用户点过气泡后清空累积列表 */
  function clear() {
    changes.value = []
  }

  /** 手动移除单条（重命名场景下避免重复提醒） */
  function dismissByFsId(fs_id: number) {
    const idx = changes.value.findIndex((c) => c.fs_id === fs_id)
    if (idx >= 0) {
      changes.value.splice(idx, 1)
    }
  }

  /** system 事件路由 —— 直接走 onSystemEvent 拿到的就是 SystemEvent */
  function handleSystemEvent(evt: SystemEvent): void {
    if (evt.event_type === 'remote_file_changed') {
      pushChange(evt)
    }
  }

  function subscribe(): void {
    const wsClient = getWebSocketClient()
    if (!wsClient) {
      // 不致命 —— 没 WS 客户端就静默退出，composable 仍可被用 clear 等手动操作
      return
    }
    try {
      wsClient.subscribe(['system'])
    } catch {
      // 客户端可能在重连，吞掉即可
    }
    unsubMessage = wsClient.onSystemEvent(handleSystemEvent)
    unsubState = wsClient.onConnectionStateChange((s) => {
      connectionState.value = s
    })
    // 同步一次初值
    connectionState.value = wsClient.getConnectionState()
  }

  function unsubscribe(): void {
    const wsClient = getWebSocketClient()
    if (wsClient) {
      try {
        wsClient.unsubscribe(['system'])
      } catch {
        /* same as above */
      }
    }
    unsubMessage?.()
    unsubMessage = null
    unsubState?.()
    unsubState = null
  }

  onMounted(subscribe)
  onUnmounted(unsubscribe)

  return {
    changes,
    connectionState,
    clear,
    dismissByFsId,
  }
}
