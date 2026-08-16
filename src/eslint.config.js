import js from "@eslint/js";
import tseslint from "typescript-eslint";
import pluginVue from "eslint-plugin-vue";
import vueParser from "vue-eslint-parser";

export default [
  js.configs.recommended,
  ...tseslint.configs.recommendedTypeChecked, // 开启严格类型检查
  ...pluginVue.configs["flat/strongly-recommended"], // Vue 官方强推规则
  {
    files: ["**/*.vue"],
    languageOptions: {
      parser: vueParser,
      parserOptions: { parser: tseslint.parser, project: "./tsconfig.json" },
    },
  },
  {
    rules: {
      // 强制 Vue 官方风格指南核心规则
      "vue/multi-word-component-names": "error",
      "vue/no-v-html": "warn", // 防 XSS
      "@typescript-eslint/no-explicit-any": "error",
      "@typescript-eslint/consistent-type-imports": "error", // 强制 type import
    },
  },
];
