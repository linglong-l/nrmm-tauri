/**
 * i18n国际化配置模块
 *
 * 职责：
 * - 初始化vue-i18n实例
 * - 加载多语言翻译文件
 * - 配置默认语言和回退策略
 *
 * 支持语言：
 * - zh-CN: 简体中文（默认）
 * - en: 英语（回退语言，翻译缺失时使用）
 * - zh-TW: 繁体中文
 * - ru: 俄语
 * - id: 印尼语
 *
 * 使用Composition API模式（legacy: false），配合useI18n()在组件中使用
 */
import { createI18n } from 'vue-i18n'
/** 英语翻译 */
import en from './en.json'
/** 简体中文翻译 */
import zhCN from './zh-CN.json'
/** 繁体中文翻译 */
import zhTW from './zh-TW.json'
/** 俄语翻译 */
import ru from './ru.json'
/** 印尼语翻译 */
import id from './id.json'

/** 支持的语言代码类型 */
export type Locale = 'en' | 'zh-CN' | 'zh-TW' | 'ru' | 'id'

/**
 * i18n实例配置
 *
 * 配置说明：
 * - legacy: false - 使用Composition API模式，支持<script setup>
 * - locale: 'zh-CN' - 默认语言为简体中文
 * - fallbackLocale: 'en' - 翻译缺失时回退到英语
 * - messages - 加载的所有语言翻译包
 */
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
