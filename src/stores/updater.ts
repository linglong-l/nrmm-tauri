import { defineStore } from 'pinia';
import { computed, ref } from 'vue';
import { listen } from '@tauri-apps/api/event';
import {
  invokeCheckUpdate,
  invokeDownloadAndInstallUpdate,
  invokeRestartApp
} from '../utils/invoke';
import { createLogger } from '../utils/logger';
import type { UpdaterStatus, UpdateAvailableInfo } from '../types';

const log = createLogger('Updater');

interface UpdateAvailablePayload {
  version: string;
  body: string;
  date: string;
}

interface UpdateDownloadProgressPayload {
  chunkLength: number;
  contentLength: number;
}

interface UpdateStatusPayload {
  [key: string]: unknown;
  status: string;
  error?: string;
  version?: string;
}

export const useUpdaterStore = defineStore('updater', () => {
  const status = ref<UpdaterStatus>('idle');
  const updateInfo = ref<UpdateAvailableInfo | null>(null);
  const errorMessage = ref('');
  const downloadedBytes = ref(0);
  const totalBytes = ref(0);
  const contentLength = ref<number | null>(null);

  let listenersSetup = false;

  const isChecking = computed(() => status.value === 'checking');
  const isDownloading = computed(() => status.value === 'downloading');
  const isInstalling = computed(() => status.value === 'installing');
  const isBusy = computed(() => ['checking', 'downloading', 'installing'].includes(status.value));
  const hasUpdate = computed(() => status.value === 'available');
  const needsRestart = computed(() => status.value === 'done');
  const progressPercent = computed(() => {
    const total = totalBytes.value || contentLength.value || 0;
    if (!total) return 0;
    return Math.min(100, Math.round((downloadedBytes.value / total) * 100));
  });

  function setupListeners() {
    if (listenersSetup) return;
    listenersSetup = true;

    listen<UpdateAvailablePayload>('tauri://update-available', (event) => {
      const payload = event.payload;
      log.info(`Update available: v${payload.version}`);
      updateInfo.value = {
        version: payload.version,
        body: payload.body || '',
        date: payload.date || ''
      };
      status.value = 'available';
    }).catch((e) => {
      log.warn('Failed to listen tauri://update-available', { reason: String(e), impact: 'Update available events will not be received' });
    });

    listen<UpdateDownloadProgressPayload>('tauri://update-download-progress', (event) => {
      const p = event.payload;
      if (p.contentLength) {
        contentLength.value = p.contentLength;
        totalBytes.value = p.contentLength;
      }
      downloadedBytes.value += p.chunkLength || 0;
      if (status.value !== 'downloading') {
        status.value = 'downloading';
      }
    }).catch((e) => {
      log.warn('Failed to listen tauri://update-download-progress', { reason: String(e), impact: 'Download progress will not be shown' });
    });

    listen<UpdateStatusPayload>('tauri://update-status', (event) => {
      const p = event.payload;
      log.info(`Update status: ${p.status}`, p as Record<string, unknown>);
      switch (p.status) {
        case 'installing':
          status.value = 'installing';
          downloadedBytes.value = 0;
          totalBytes.value = contentLength.value || 0;
          break;
        case 'done':
          status.value = 'done';
          break;
        case 'error':
          status.value = 'error';
          errorMessage.value = (p.error as string) || 'Unknown error';
          break;
        case 'not-available':
          status.value = 'up-to-date';
          break;
        case 'downloading':
          status.value = 'downloading';
          break;
        default:
          log.warn(`Unknown update status: ${p.status}`, { reason: 'Unknown status', impact: 'Status display may be incorrect' });
      }
    }).catch((e) => {
      log.warn('Failed to listen tauri://update-status', { reason: String(e), impact: 'Update status events will not be received' });
    });
  }

  async function check(): Promise<boolean> {
    setupListeners();
    if (isBusy.value) return false;
    status.value = 'checking';
    errorMessage.value = '';
    try {
      const res = await invokeCheckUpdate();
      if (!res.ok) {
        status.value = 'error';
        errorMessage.value = res.error;
        return false;
      }
      if (!res.data) {
        status.value = 'up-to-date';
        updateInfo.value = null;
        return false;
      }
      return true;
    } catch (e) {
      status.value = 'error';
      errorMessage.value = String(e);
      return false;
    }
  }

  async function confirmInstall(): Promise<void> {
    if (status.value !== 'available') return;
    status.value = 'downloading';
    downloadedBytes.value = 0;
    totalBytes.value = 0;
    contentLength.value = null;
    errorMessage.value = '';
    const res = await invokeDownloadAndInstallUpdate();
    if (!res.ok) {
      status.value = 'error';
      errorMessage.value = res.error;
    }
  }

  async function restartApp(): Promise<void> {
    await invokeRestartApp();
  }

  function dismiss(): void {
    if (status.value === 'error' || status.value === 'up-to-date') {
      reset();
    }
  }

  function reset(): void {
    status.value = 'idle';
    updateInfo.value = null;
    errorMessage.value = '';
    downloadedBytes.value = 0;
    totalBytes.value = 0;
    contentLength.value = null;
  }

  return {
    status, updateInfo, errorMessage, downloadedBytes, totalBytes, contentLength,
    isChecking, isDownloading, isInstalling, isBusy, hasUpdate, needsRestart, progressPercent,
    check, confirmInstall, restartApp, dismiss, reset
  };
});
