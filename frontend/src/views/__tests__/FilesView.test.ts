import { mount } from '@vue/test-utils'
import { nextTick, ref } from 'vue'
import ElementPlus from 'element-plus'
import { describe, expect, it, vi } from 'vitest'
import FilesView from '../FilesView.vue'

vi.mock('@/utils/responsive', () => ({
  useIsMobile: () => ref(false),
}))

vi.mock('@/api/file', () => ({
  getFileList: vi.fn().mockResolvedValue({
    list: [],
    has_more: false,
    page: 1,
  }),
  searchFiles: vi.fn().mockResolvedValue({
    list: [],
    has_more: false,
  }),
  formatFileSize: vi.fn((size: number) => `${size}B`),
  formatTime: vi.fn(() => '2026-04-10 00:00:00'),
  createFolder: vi.fn(),
  joinPath: vi.fn((parent: string, name: string) => (parent === '/' ? `/${name}` : `${parent}/${name}`)),
  basename: vi.fn((p: string) => p.slice(p.lastIndexOf('/') + 1)),
}))

vi.mock('@/api/download', () => ({
  createDownload: vi.fn(),
  createFolderDownload: vi.fn(),
  createBatchDownload: vi.fn(),
}))

vi.mock('@/api/upload', () => ({
  createUpload: vi.fn(),
  createFolderUpload: vi.fn(),
}))

vi.mock('@/api/config', () => ({
  getConfig: vi.fn().mockResolvedValue({
    download: {
      recent_directory: '',
      default_directory: '',
      download_dir: 'downloads',
      ask_each_time: false,
    },
    upload: {
      recent_directory: '',
    },
    conflict_strategy: {
      default_upload_strategy: 'smart_dedup',
      default_download_strategy: 'overwrite',
    },
  }),
  updateRecentDirDebounced: vi.fn(),
  setDefaultDownloadDir: vi.fn(),
}))

vi.mock('@/api/autobackup', () => ({
  getEncryptionStatus: vi.fn().mockResolvedValue({
    has_key: false,
  }),
}))

describe('FilesView 桌面端搜索浮层', () => {
  it('展开搜索时保留图标按钮，并在空输入时点击外部自动收起', async () => {
    const wrapper = mount(FilesView, {
      attachTo: document.body,
      global: {
        plugins: [ElementPlus],
        stubs: {
          HomeFilled: true,
          Search: true,
          Download: true,
          Link: true,
          FolderAdd: true,
          Upload: true,
          Share: true,
          Refresh: true,
          Folder: true,
          Document: true,
          Loading: true,
          'el-table': true,
          'el-table-column': true,
          FilePickerModal: true,
          TransferDialog: true,
          ShareDialog: true,
          ShareDirectDownloadDialog: true,
        },
      },
    })

    await Promise.resolve()
    await Promise.resolve()
    await nextTick()

    const toggle = wrapper.find('.search-flyout-toggle')
    expect(toggle.exists()).toBe(true)

    await toggle.trigger('click')
    await nextTick()

    expect(wrapper.find('.search-flyout-toggle').exists()).toBe(true)
    expect(wrapper.find('.search-flyout-panel').exists()).toBe(true)

    document.body.dispatchEvent(new MouseEvent('mousedown', { bubbles: true }))
    await nextTick()

    expect(wrapper.find('.search-flyout-panel').exists()).toBe(false)

    wrapper.unmount()
  })
})

describe('FilesView 路径面包屑', () => {
  function mountFilesView() {
    return mount(FilesView, {
      attachTo: document.body,
      global: {
        plugins: [ElementPlus],
        stubs: {
          HomeFilled: true,
          Search: true,
          Download: true,
          Link: true,
          FolderAdd: true,
          Upload: true,
          Share: true,
          Refresh: true,
          Folder: true,
          Document: true,
          Loading: true,
          'el-table': true,
          'el-table-column': true,
          FilePickerModal: true,
          TransferDialog: true,
          ShareDialog: true,
          ShareDirectDownloadDialog: true,
        },
      },
    })
  }

  it('只给当前层级打 is-last，中间层级保留分隔符', async () => {
    const wrapper = mountFilesView()
    await nextTick()

    // @ts-expect-error setup 内部状态，测试中直接驱动
    wrapper.vm.currentDir = '/产品/播德海/档案数字化加工平台'
    await nextTick()

    const items = wrapper.findAll('.breadcrumb-bar .el-breadcrumb__item')
    // 根目录 + 三级路径
    expect(items).toHaveLength(4)
    expect(items.map(i => i.classes('is-last'))).toEqual([false, false, false, true])
    // 分隔符始终在 DOM 中，可见性由 is-last 类决定，不依赖 :last-child
    expect(wrapper.findAll('.breadcrumb-bar .el-breadcrumb__separator')).toHaveLength(4)

    wrapper.unmount()
  })

  it('点击中间层级跳转，点击分隔符不跳转', async () => {
    const { getFileList } = await import('@/api/file')
    const wrapper = mountFilesView()
    await nextTick()

    // @ts-expect-error setup 内部状态，测试中直接驱动
    wrapper.vm.currentDir = '/产品/播德海/档案数字化加工平台'
    await nextTick()

    const items = wrapper.findAll('.breadcrumb-bar .el-breadcrumb__item')
    vi.mocked(getFileList).mockClear()

    // 分隔符不是点击热区
    await items[1].find('.el-breadcrumb__separator').trigger('click')
    expect(getFileList).not.toHaveBeenCalled()

    // 层级文字可点击，跳到对应层级
    await items[1].find('.crumb-link').trigger('click')
    await nextTick()
    expect(getFileList).toHaveBeenCalled()
    expect(vi.mocked(getFileList).mock.calls[0][0]).toBe('/产品')

    wrapper.unmount()
  })
})
