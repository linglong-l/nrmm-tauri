import { createI18n } from 'vue-i18n';
import en from '../assets/translations/en.json';
import zhCN from '../assets/translations/zh-CN.json';
import zhTW from '../assets/translations/zh-TW.json';
import id from '../assets/translations/id.json';
import ru from '../assets/translations/ru.json';

export const i18n = createI18n({
  legacy: false,
  locale: 'en',
  fallbackLocale: 'en',
  messages: {
    en,
    'zh-CN': zhCN,
    'zh-TW': zhTW,
    id,
    ru,
  },
});
