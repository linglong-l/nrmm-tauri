import { defineStore } from 'pinia';
import { ref } from 'vue';
import { invokeGetVersionInfo } from '../utils/invoke';
import { createLogger } from '../utils/logger';

const log = createLogger('Version');

export const useVersionStore = defineStore('version', () => {
  const version = ref('0.0.0');
  const commit = ref<string | null>(null);
  const buildDate = ref<string | null>(null);
  const isLoaded = ref(false);

  async function load(): Promise<void> {
    if (isLoaded.value) return;
    const res = await invokeGetVersionInfo();
    if (res.ok) {
      version.value = res.data.version;
      commit.value = res.data.commit;
      buildDate.value = res.data.buildDate;
      isLoaded.value = true;
      log.info(`Version loaded: v${version.value}`);
    } else {
      log.warn('Failed to load version info', { reason: res.error, impact: 'Version will show as unknown' });
      version.value = 'unknown';
    }
  }

  return { version, commit, buildDate, isLoaded, load };
});
