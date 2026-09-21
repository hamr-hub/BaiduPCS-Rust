// 应用版本号：唯一来源是 package.json 的 version 字段，
// 由 vite.config.ts 的 define 在编译期注入到全局常量 __APP_VERSION__。
// 界面需要显示版本号时统一从这里引用，不要再硬编码。
export const APP_VERSION = __APP_VERSION__

// 带 "v" 前缀的展示用版本号（形如 "vX.Y.Z"）
export const APP_VERSION_LABEL = `v${__APP_VERSION__}`
