import { ref } from 'vue'
import { getPlatformInfo } from '../utils/tauri'
import { logger } from '../utils/logger'
import type { PlatformInfo } from '../types'

const platformInfo = ref<PlatformInfo | null>(null)
const initialized = ref(false)

export async function initPlatform() {
  try {
    platformInfo.value = await getPlatformInfo()
    initialized.value = true
  } catch (e) {
    logger.error('Platform', 'Failed to get platform info', e)
    platformInfo.value = {
      os: 'windows',
      desktopSession: '',
      isWayland: false,
      isX11: false,
      isWslg: false,
      transparencySupported: true,
    }
    initialized.value = true
  }
}

export function usePlatform() {
  return { platformInfo, initialized }
}
