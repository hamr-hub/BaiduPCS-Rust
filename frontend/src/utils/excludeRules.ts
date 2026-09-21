/**
 * 分享同步「排除规则」输入解析工具。
 *
 * 被订阅编辑器（ShareIncludeExcludeEditor）与系统设置页（常用词表）共用，
 * 保证两处输入行为一致。
 *
 * 设计约束：**换行是唯一的批量分隔符。**
 * - 逗号/分号/顿号都是合法的文件名字符（Windows/百度网盘仅禁止 `\/:*?"<>|`），
 *   拿它们当分隔符会把 `*第1，2集*` 这类真实规则静默切坏；换行不可能出现在
 *   文件名里，永不冲突。
 * - 输入框用 textarea：回车提交当前内容（逐条输入），粘贴多行文本则每行一条。
 * - 不按空格切分 —— 规则常含空格（如 `*S02 连载中*`）。
 */

/** 把输入文本解析成规则列表：按行拆分，行内内容（含逗号等标点）原样保留。 */
export function parseRuleInput(text: string): string[] {
  return text
    .split(/[\r\n]+/)
    .map((s) => s.trim())
    .filter(Boolean)
}

/** 把若干规则合并进已有列表：去重、保序，返回新数组（不修改入参） */
export function mergeRules(existing: readonly string[], incoming: readonly string[]): string[] {
  const next = [...existing]
  for (const p of incoming) {
    const t = p.trim()
    if (t && !next.includes(t)) next.push(t)
  }
  return next
}
