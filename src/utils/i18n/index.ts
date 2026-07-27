import { createI18n } from 'vue-i18n'
import en from './en.json'
import zhCN from './zh-CN.json'
import zhTW from './zh-TW.json'
import ru from './ru.json'
import id from './id.json'

export type Locale = 'en' | 'zh-CN' | 'zh-TW' | 'ru' | 'id'

const i18n = createI18n({
  legacy: false,
  locale: 'zh-CN',
  fallbackLocale: 'en',
  messages: {
    'en': en,
    'zh-CN': zhCN,
    'zh-TW': zhTW,
    'ru': ru,
    'id': id,
  },
})

export default i18n
