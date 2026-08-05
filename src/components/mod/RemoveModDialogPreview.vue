<template>
  <div class="preview-root">
    <div class="preview-header">
      <h2 class="preview-title">移除模组对话框 · 快速验证工具</h2>
      <div class="preview-hint">
        右键卡片 → 移除模组，或点击上方快捷按钮直接进入目标状态
      </div>
    </div>

    <!-- 快捷按钮：一键直达各状态 -->
    <div class="preview-shortcuts">
      <button class="sc-btn" @click="state = 'view1-short'">UI1 · 短名</button>
      <button class="sc-btn" @click="state = 'view1-long'">UI1 · 长英文名</button>
      <button class="sc-btn" @click="state = 'view1-cn'">UI1 · 长中文名</button>
      <button class="sc-btn" @click="state = 'view1-special'">UI1 · 特殊字符</button>
      <button class="sc-btn sc-success" @click="state = 'view2'">UI2 · 成功提示</button>
      <button class="sc-btn sc-error" @click="state = 'view-err'">错误态 · 示例</button>
      <button class="sc-btn sc-loading" @click="state = 'view-loading'">加载态 · 示例</button>
      <button class="sc-btn sc-close" @click="state = ''">关闭</button>
    </div>

    <!-- 模拟模组卡片网格（右键 → 移除模组 → 真实 RemoveModDialog confirm 态） -->
    <div class="mock-grid">
      <div
        v-for="m in mockMods"
        :key="m.id"
        class="mock-card"
        @contextmenu.prevent="onCardRightClick(m, $event)"
        @click="onCardLeftClick(m)"
      >
        <div class="mock-badges">
          <span v-if="m.fav" class="mock-badge fav">⭐</span>
          <span v-if="m.disabled" class="mock-badge dis">🔒</span>
        </div>
        <div class="mock-image" :style="{ background: m.color }">
          <span class="mock-image-placeholder">👗</span>
        </div>
        <div class="mock-name" :title="m.name">{{ m.name }}</div>
      </div>
    </div>

    <!-- 卡片右键菜单 -->
    <Teleport to="body">
      <div
        v-if="ctxMenuVisible"
        class="mock-ctx-menu"
        :style="{ top: ctxMenuY + 'px', left: ctxMenuX + 'px' }"
        @click.stop
      >
        <div class="mock-ctx-item" @click="ctxToggleFav">
          {{ selected?.fav ? '取消收藏' : '收藏' }}
        </div>
        <div class="mock-ctx-item" @click="ctxToggleDisabled">
          {{ selected?.disabled ? '启用模组' : '禁用模组' }}
        </div>
        <div class="mock-ctx-divider"></div>
        <div class="mock-ctx-item" @click="ctxRename">重命名</div>
        <div class="mock-ctx-divider"></div>
        <div class="mock-ctx-item danger" @click="ctxRemove">移除模组</div>
      </div>
    </Teleport>

    <!-- ========== 4 种状态直接渲染（快捷按钮触发） ========== -->

    <!-- UI1 · 确认对话框（通用模板，显示的模组名称由 state 决定） -->
    <Teleport to="body">
      <div v-if="state && state.startsWith('view1-')" class="ovr" @click.self="state = ''">
        <div class="dlg">
          <div class="dlg-title">{{ t('removeMod.title') }}</div>
          <div class="dlg-name">{{ view1ModName }}</div>
          <div class="dlg-warning">{{ t('removeMod.warning') }}</div>
          <div class="dlg-hint">{{ t('removeMod.folderMovedTo') }}</div>
          <div class="dlg-footer">
            <button class="btn btn-cancel" @click="state = ''">{{ t('removeMod.cancel') }}</button>
            <button class="btn btn-confirm" @click="state = 'view2'">{{ t('removeMod.confirm') }}</button>
          </div>
        </div>
      </div>
    </Teleport>

    <!-- UI2 · 成功提示框 -->
    <Teleport to="body">
      <div v-if="state === 'view2'" class="ovr" @click.self="state = ''">
        <div class="dlg">
          <div class="dlg-title is-bold">{{ t('removeMod.title') }}</div>
          <div class="dlg-ok">{{ t('removeMod.success.folderMoved') }}</div>
          <div class="dlg-ok">{{ t('removeMod.success.modRestored') }}</div>
          <div class="dlg-footer">
            <button class="btn btn-cancel" @click="handleView2Confirm">{{ t('removeMod.confirm') }}</button>
          </div>
        </div>
      </div>
    </Teleport>

    <!-- 加载态 -->
    <Teleport to="body">
      <div v-if="state === 'view-loading'" class="ovr">
        <div class="dlg">
          <div class="dlg-title">{{ t('removeMod.title') }}</div>
          <div class="dlg-loading">
            <span>{{ t('removeMod.loading') }}</span>
            <span v-for="n in loadingDots" :key="n">.</span>
          </div>
          <div class="dlg-footer">
            <button class="btn btn-cancel" :disabled="true">{{ t('removeMod.cancel') }}</button>
            <button class="btn btn-confirm" :disabled="true">{{ t('removeMod.confirm') }}</button>
          </div>
        </div>
      </div>
    </Teleport>

    <!-- 错误态 -->
    <Teleport to="body">
      <div v-if="state === 'view-err'" class="ovr" @click.self="state = ''">
        <div class="dlg">
          <div class="dlg-title">{{ t('removeMod.title') }}</div>
          <div class="dlg-error">
            <span class="dlg-error-icon">✕</span>
            <span>示例错误信息：无法访问目标路径，进程被占用或权限不足。</span>
          </div>
          <div class="dlg-footer">
            <button class="btn btn-cancel" @click="state = ''">{{ t('removeMod.confirm') }}</button>
          </div>
        </div>
      </div>
    </Teleport>

    <!-- 真实 RemoveModDialog：由卡片右键 → 移除模组触发（非快捷按钮） -->
    <RemoveModDialog
      v-model="realDialogVisible"
      :mod-name="realDialogModName"
      :mod-path="realDialogModPath"
      @removed="onRealRemoved"
    />
  </div>
</template>

<script setup lang="ts">
/**
 * 移除模组对话框 · 快速验证工具
 *
 * 场景覆盖：
 * - UI1 确认框：短/长英文/长中文/特殊字符 共 4 种名称边界
 * - UI2 成功提示框
 * - 加载态（省略号动画）
 * - 错误态
 * - 真实卡片右键 → 移除模组 → 真实 RemoveModDialog 完整交互
 *
 * 使用方式（二选一）：
 *   A. 临时挂载：在 ModsView.vue <template> 底部追加一行
 *        <RemoveModDialogPreview v-if="$route?.query?.preview === '1' || true" />
 *   B. 按 Ctrl+Shift+M 打开（若父视图绑定了该快捷键）
 */
import { ref, computed, onMounted, onBeforeUnmount, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import RemoveModDialog from '@/components/mod/RemoveModDialog.vue'
import { ElMessage } from 'element-plus'

const { t } = useI18n()

type StateKey = '' | 'view1-short' | 'view1-long' | 'view1-cn' | 'view1-special' | 'view2' | 'view-loading' | 'view-err'
const state = ref<StateKey>('')

const view1ModName = computed(() => {
  switch (state.value) {
    case 'view1-short': return 'Phrolova 白裙'
    case 'view1-long': return 'Phrolova_WhiteDress_3DMigoto_Special_Edition_v2.5_Final'
    case 'view1-cn': return '芙宁娜·德·枫丹-极夜真梦·神装【4K实机渲染】'
    case 'view1-special': return 'Mod with & chars <test> (v1.0) [FIXED]'
    default: return ''
  }
})

const loadingDots = ref(1)
let dotsTimer: ReturnType<typeof setInterval> | null = null
watch(state, (v) => {
  if (v === 'view-loading') {
    if (dotsTimer) clearInterval(dotsTimer)
    dotsTimer = setInterval(() => {
      loadingDots.value = (loadingDots.value % 6) + 1
    }, 300)
  } else if (dotsTimer) {
    clearInterval(dotsTimer)
    dotsTimer = null
  }
}, { immediate: true })
onBeforeUnmount(() => { if (dotsTimer) clearInterval(dotsTimer) })

function handleView2Confirm() {
  state.value = ''
  // emit 'removed' 语义：模组重读取触发
  ElMessage.success('（模拟）removed 事件已发射 → 即将模组重读取')
}

// ===== 模拟模组数据 =====
interface MockMod {
  id: string
  name: string
  color: string
  fav: boolean
  disabled: boolean
  modPath: string
}
const mockMods: MockMod[] = [
  {
    id: '1', name: 'Phrolova 白裙',
    color: 'linear-gradient(135deg,#f5f7fa 0%,#e4ecf7 100%)',
    fav: true, disabled: false,
    modPath: 'D:\\Games\\GIMI\\Mods\\_MANAGED_\\group_01\\Phrolova_WhiteDress'
  },
  {
    id: '2', name: '芙宁娜·极夜真梦神装【4K】',
    color: 'linear-gradient(135deg,#2c3e50 0%,#4c6b8a 100%)',
    fav: true, disabled: false,
    modPath: 'D:\\Games\\GIMI\\Mods\\_MANAGED_\\group_01\\Furina_PolarNight_Dream'
  },
  {
    id: '3', name: '申鹤-清凉夏日版 v2.1',
    color: 'linear-gradient(135deg,#f093fb 0%,#f5576c 100%)',
    fav: false, disabled: true,
    modPath: 'D:\\Games\\GIMI\\Mods\\_MANAGED_\\group_02\\DISABLED_Shenhe_Summer_v2'
  },
  {
    id: '4', name: '非常非常非常非常非常非常非常长的模组名称测试边界',
    color: 'linear-gradient(135deg,#667eea 0%,#764ba2 100%)',
    fav: false, disabled: false,
    modPath: 'D:\\Games\\GIMI\\Mods\\_MANAGED_\\group_02\\VeryLongNameForTest'
  },
  {
    id: '5', name: 'Special & chars (test) [FIXED] v1.0',
    color: 'linear-gradient(135deg,#43e97b 0%,#38f9d7 100%)',
    fav: false, disabled: false,
    modPath: 'D:\\Games\\GIMI\\Mods\\_MANAGED_\\group_03\\Special_Chars_v1'
  },
  {
    id: '6', name: '八重神子-礼服',
    color: 'linear-gradient(135deg,#fa709a 0%,#fee140 100%)',
    fav: true, disabled: false,
    modPath: 'D:\\Games\\GIMI\\Mods\\_MANAGED_\\group_03\\YaeMiko_FormalDress'
  }
]

// ===== 右键菜单 =====
const ctxMenuVisible = ref(false)
const ctxMenuX = ref(0)
const ctxMenuY = ref(0)
const selected = ref<MockMod | null>(null)

function onCardLeftClick(m: MockMod) {
  selected.value = m
  if (ctxMenuVisible.value) ctxMenuVisible.value = false
}

function onCardRightClick(m: MockMod, e: MouseEvent) {
  selected.value = m
  const W = 180, H = 210
  const vw = window.innerWidth, vh = window.innerHeight
  let x = e.clientX, y = e.clientY
  if (x + W > vw - 4) x = vw - W - 4
  if (y + H > vh - 4) y = vh - H - 4
  ctxMenuX.value = x
  ctxMenuY.value = y
  ctxMenuVisible.value = true
}
function closeCtx() { ctxMenuVisible.value = false }

function ctxToggleFav() { if (selected.value) selected.value.fav = !selected.value.fav; closeCtx() }
function ctxToggleDisabled() { if (selected.value) selected.value.disabled = !selected.value.disabled; closeCtx() }
function ctxRename() {
  if (!selected.value) return
  const n = prompt('重命名模组：', selected.value.name)
  if (n) selected.value.name = n
  closeCtx()
}

// ===== 真实 RemoveModDialog 触发 =====
const realDialogVisible = ref(false)
const realDialogModName = ref('')
const realDialogModPath = ref('')

function ctxRemove() {
  closeCtx()
  if (!selected.value) return
  realDialogModName.value = selected.value.name
  realDialogModPath.value = selected.value.modPath
  realDialogVisible.value = true
}

function onRealRemoved() {
  ElMessage.success('removed 事件发射 → 已触发模组重读取')
}

function onDocClick() { if (ctxMenuVisible.value) ctxMenuVisible.value = false }
onMounted(() => document.addEventListener('click', onDocClick))
onBeforeUnmount(() => document.removeEventListener('click', onDocClick))
</script>

<style scoped>
.preview-root {
  position: fixed; inset: 32px; z-index: 9990;
  background: rgba(26, 24, 31, 0.97);
  border-radius: 12px; border: 1px solid rgba(255,255,255,0.08);
  padding: 24px; overflow: auto; color: #fff;
  font-family: -apple-system, "Segoe UI", Roboto, sans-serif;
}
.preview-header { margin-bottom: 18px; border-bottom: 1px solid rgba(255,255,255,0.08); padding-bottom: 14px; }
.preview-title { margin: 0 0 6px; font-size: 18px; font-weight: 700; }
.preview-hint { font-size: 12px; color: #909399; }

.preview-shortcuts { display: flex; flex-wrap: wrap; gap: 8px; margin-bottom: 24px; }
.sc-btn {
  padding: 7px 14px; font-size: 12px; font-weight: 500;
  background: rgba(69,75,93,0.5); color: #c0c4cc;
  border: 1px solid rgba(255,255,255,0.08);
  border-radius: 999px; cursor: pointer; transition: all 0.15s ease;
}
.sc-btn:hover { background: #454b5d; color: #fff; }
.sc-success { color: #67c23a; border-color: rgba(103,194,58,0.3); }
.sc-success:hover { background: rgba(103,194,58,0.18); color: #85ce61; }
.sc-error { color: #f56c6c; border-color: rgba(245,108,108,0.3); }
.sc-error:hover { background: rgba(245,108,108,0.18); color: #f78989; }
.sc-loading { color: #409eff; border-color: rgba(64,158,255,0.3); }
.sc-loading:hover { background: rgba(64,158,255,0.18); color: #66b1ff; }
.sc-close { color: #909399; }

.mock-grid {
  display: grid; grid-template-columns: repeat(auto-fill, minmax(110px, 1fr));
  gap: 14px;
}
.mock-card {
  position: relative; border-radius: 10px;
  border: 1px solid rgba(255,255,255,0.1); overflow: hidden;
  cursor: pointer; transition: transform 0.15s ease, border-color 0.15s ease;
  background: rgba(255,255,255,0.02); user-select: none;
}
.mock-card:hover { transform: translateY(-2px); border-color: rgba(74,158,255,0.4); }
.mock-badges { position: absolute; top: 6px; left: 6px; z-index: 2; display: flex; gap: 4px; }
.mock-badge { font-size: 12px; filter: drop-shadow(0 1px 2px rgba(0,0,0,0.6)); }
.mock-image { aspect-ratio: 100 / 140; display: flex; align-items: center; justify-content: center; }
.mock-image-placeholder { font-size: 36px; filter: drop-shadow(0 2px 6px rgba(0,0,0,0.3)); }
.mock-name {
  padding: 8px 10px 10px; font-size: 12px; color: #e4e7ed;
  line-height: 1.4; text-align: center; white-space: nowrap;
  overflow: hidden; text-overflow: ellipsis;
}
.mock-ctx-menu {
  position: fixed; z-index: 10000; min-width: 170px;
  background: rgba(43,41,48,0.98);
  border: 1px solid rgba(255,255,255,0.08);
  border-radius: 10px; padding: 6px;
  box-shadow: 0 8px 24px rgba(0,0,0,0.5);
}
.mock-ctx-item {
  padding: 8px 12px; font-size: 13px; color: #e4e7ed;
  border-radius: 6px; cursor: pointer; display: flex;
  align-items: center; gap: 8px; transition: background 0.12s ease;
}
.mock-ctx-item:hover { background: rgba(74,158,255,0.14); color: #fff; }
.mock-ctx-item.danger { color: #f56c6c; }
.mock-ctx-item.danger:hover { background: rgba(245,108,108,0.14); color: #f78989; }
.mock-ctx-divider { height: 1px; background: rgba(255,255,255,0.08); margin: 4px 2px; }

/* ===== 对话框覆盖层 ===== */
.ovr {
  position: fixed; inset: 0; z-index: 9999;
  background: rgba(0,0,0,0.45);
  display: flex; align-items: center; justify-content: center;
  pointer-events: all;
}
.dlg {
  background: #2b2930; border-radius: 12px;
  padding: 24px 28px 20px;
  min-width: 380px; max-width: 480px;
  color: #fff;
  box-shadow: 0 8px 32px rgba(0,0,0,0.6);
}
.dlg-title {
  color: #fff; font-size: 18px; font-weight: 500;
  margin-bottom: 16px; line-height: 1.4;
}
.dlg-title.is-bold { font-weight: 700; }
.dlg-name {
  color: #fff; font-size: 14px; line-height: 1.6;
  margin-bottom: 14px; word-break: break-all;
}
.dlg-warning {
  color: #ffc107; font-size: 13px; line-height: 1.6;
  margin-bottom: 12px;
}
.dlg-hint {
  color: #909399; font-size: 12px; line-height: 1.6;
  margin-bottom: 18px;
}
.dlg-loading { padding: 8px 0 4px; }
.dlg-loading > span:first-child { color: #fff; font-size: 14px; }
.dlg-ok { color: #67c23a; font-size: 14px; line-height: 1.8; font-weight: 500; }
.dlg-error {
  display: flex; align-items: center; gap: 8px;
  font-size: 14px; margin-bottom: 18px;
}
.dlg-error-icon { color: #f56c6c; font-weight: 700; }
.dlg-error > span:last-child { color: #f56c6c; word-break: break-all; }

.dlg-footer {
  display: flex; justify-content: flex-end;
  align-items: center; gap: 8px; margin-top: 18px;
}
.btn {
  padding: 8px 20px; font-size: 13px; font-weight: 600;
  background: transparent; border: none;
  border-radius: 999px; cursor: pointer;
  transition: background-color 0.15s ease;
}
.btn:disabled { opacity: 0.5; cursor: not-allowed; }
.btn:hover:not(:disabled) { background-color: #454b5d; }
.btn-cancel { color: #409eff; }
.btn-confirm { color: #f56c6c; }
</style>
